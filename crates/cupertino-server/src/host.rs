//! Shell host protocol (`cupertino_host_v1`, `cupertino_host_component_v1`, `cupertino_component_v1`),
//! per-component connections and roles, host bus bridges, D20 bus identity nonce verification,
//! D16/D17 liveness and failure restart tracking, and D19 persistent surface holds.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use cupertino_peer::{Caller, Denial, ProcessHandle};
use cupertino_protocols::server::host::cupertino_component_v1::{
    self, BusNameResult, CupertinoComponentV1, Error as ComponentError,
};
use cupertino_protocols::server::host::cupertino_host_component_v1::{
    self, CupertinoHostComponentV1, Error as HostComponentError, StopReason,
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
    /// Last captured shm pixel buffer for hold display if the connection drops.
    last_picture: Option<HeldPicture>,
}

/// Retained CPU pixel buffer of a held persistent surface.
#[derive(Clone)]
pub struct HeldPicture {
    pub buffer: MemoryRenderBuffer,
    pub size: (i32, i32),
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
    /// If the server expects this host to exit, the deadline after which it is killed via pidfd.
    expected_exit_kill_at: Option<Instant>,
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
    restart_waiters: Vec<(Instant, RestartWaiter)>,
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
    held_wm_outputs: HashSet<OutputId>,
    /// Shared-host unexpected crash timestamps (3 within 60s isolates all components).
    shared_host_crashes: Vec<Instant>,
    /// Once 3 shared-host crashes occur within 60s, every component runs in a separate host.
    pub all_separate_for_session: bool,
    /// Last time seat input occurred (for shared-host `request_restart` quiet-moment check).
    last_input_at: Instant,
    /// Separate-host component names the server needs spawned externally (drained by supervisor/tests).
    pub requested_separate_hosts: Vec<&'static str>,
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
            held_wm_outputs: HashSet::new(),
            shared_host_crashes: Vec::new(),
            all_separate_for_session: false,
            last_input_at: Instant::now(),
            requested_separate_hosts: Vec::new(),
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

    /// Whether window-manager placements are currently held on `output`.
    pub fn wm_placements_held(&self, output: OutputId) -> bool {
        self.held_wm_outputs.contains(&output)
    }

    /// Clear held window-manager placements on `output` once the new WM commits placements or finishes takeover.
    pub fn clear_held_wm_placements(&mut self, output: OutputId) {
        self.held_wm_outputs.remove(&output);
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

    /// Snapshot the current shm pixel contents of `surface` if accessible, so a held surface keeps its last picture.
    fn capture_shm_picture(surface: &WlSurface) -> Option<HeldPicture> {
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
            let fourcc = match data.format {
                smithay::reexports::wayland_server::protocol::wl_shm::Format::Xrgb8888 => {
                    Fourcc::Xrgb8888
                }
                _ => Fourcc::Argb8888,
            };
            let mem_buf = MemoryRenderBuffer::from_slice(
                &tightly_packed,
                fourcc,
                (w, h),
                1,
                Transform::Normal,
                None,
            );
            Some(HeldPicture {
                buffer: mem_buf,
                size: (w, h),
            })
        });
        captured.ok().flatten()
    }
}

/// Generate a 128-bit cryptographically random nonce formatted as 32 lowercase hexadecimal characters.
fn generate_nonce() -> String {
    let mut bytes = [0u8; 16];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_ok();
    if !ok {
        // Fallback mixing monotonic clock and process id if /dev/urandom is unavailable.
        let t = Instant::now().elapsed().as_nanos() ^ u128::from(std::process::id());
        bytes = t.to_le_bytes();
    }
    let mut out = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
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

impl Cupertino {
    /// Register a new host connection (shared or separate), returning the client-end Wayland socket fd.
    pub fn attach_host_connection(
        &mut self,
        kind: HostKind,
        components: Vec<&'static str>,
        holder: Holder,
    ) -> std::io::Result<OwnedFd> {
        let (server_end, client_end) = UnixStream::pair()?;
        let state = ClientState {
            capabilities: ClientCapabilitySet::of([ShellRole::Host]),
            ..ClientState::default()
        };
        let process = match &holder {
            Holder::Process(p) => p.try_clone().ok(),
            Holder::Detached(_) => ProcessHandle::open(std::process::id()).ok(),
        };
        let start_ticks = process
            .as_ref()
            .and_then(cupertino_peer::component::process_start_ticks);
        let _ = state.holder.set(holder);
        let _ = state.gone.set(self.gone_clients.clone());
        state.look_up_process();
        let client = crate::state::insert_client_handle(&self.display_handle, server_end, state)?;
        let client_id = client.id();
        let host_id = self.hosts.next_host_id;
        self.hosts.next_host_id += 1;
        self.hosts.host_by_client.insert(client_id.clone(), host_id);
        self.hosts.hosts.insert(
            host_id,
            HostEntry {
                id: host_id,
                kind,
                components,
                resource: None,
                component_objects: HashMap::new(),
                connect_pending: HashSet::new(),
                restart_requested_at: None,
                expected_exit_kill_at: None,
                process,
                start_ticks,
                last_bridge_at: None,
                bridge_retry_at: None,
            },
        );
        Ok(OwnedFd::from(client_end))
    }

    /// Replace the component list of `host_id` and resend `component`…`done` if bound.
    pub fn set_host_components(&mut self, host_id: u64, components: Vec<&'static str>) {
        let now = Instant::now();
        let Some(host) = self.hosts.hosts.get_mut(&host_id) else {
            return;
        };
        host.components = components;
        if let Some(res) = host.resource.clone() {
            for &name in &host.components {
                res.component(name.to_string());
            }
            res.done();
        }
        if matches!(host.kind, HostKind::Separate(_)) && host.components.is_empty() {
            host.expected_exit_kill_at = Some(now + EXPECTED_EXIT_KILL_GRACE);
        }
        // Any inert `connect` that just entered the list can now be answered.
        let pending: Vec<&'static str> = host.connect_pending.iter().copied().collect();
        for name in pending {
            self.try_answer_component_connect(host_id, name, now);
        }
    }

    /// Ask to restart `component` (`cupertino shell restart NAME`): sends `restart` if running, or clears `gave_up`
    /// and answers a pending `connect` if stopped.
    pub fn request_component_restart(
        &mut self,
        component: &'static str,
        waiter: Option<RestartWaiter>,
    ) -> bool {
        let now = Instant::now();
        let Some(rec) = self.hosts.components.get_mut(component) else {
            return false;
        };
        rec.separate_for_session = false;
        if let Some(cb) = waiter {
            rec.restart_waiters.push((now + HOLD_READY_TIMEOUT, cb));
        }
        if rec.gave_up {
            rec.gave_up = false;
            rec.failures.clear();
            rec.consecutive_failures = 0;
            rec.backoff_until = None;
            // Find the host waiting on this component and answer connect.
            let host_id = self
                .hosts
                .hosts
                .values()
                .find(|h| h.components.contains(&component))
                .map(|h| h.id);
            if let Some(hid) = host_id {
                self.try_answer_component_connect(hid, component, now);
            }
            return true;
        }
        if let Some((_, host_id)) = rec.active {
            if let Some(obj) = self
                .hosts
                .hosts
                .get(&host_id)
                .and_then(|h| h.component_objects.get(component))
            {
                obj.restart();
                return true;
            }
        }
        false
    }

    /// Verify `org.cupertino.Session1.AnnounceComponent(nonce)` from `sender` / `caller` (D20).
    pub fn verify_component_bus_nonce(
        &mut self,
        sender: &str,
        caller: &Caller,
        nonce: &str,
    ) -> Result<(), Denial> {
        let now = Instant::now();
        let caller_proc = match caller {
            Caller::Process(id) => Some(&id.process),
            Caller::Unclassified(p) => Some(p),
            Caller::Anonymous => None,
        };

        // Locate the component that holds this pending nonce (single-use regardless of outcome).
        let target_comp = self.hosts.components.iter().find_map(|(&name, rec)| {
            rec.pending_nonce
                .as_ref()
                .is_some_and(|p| p.nonce == nonce)
                .then_some(name)
        });
        let Some(comp_name) = target_comp else {
            return Err(Denial::Untrusted);
        };
        let pending = self
            .hosts
            .components
            .get_mut(comp_name)
            .and_then(|r| r.pending_nonce.take())
            .expect("pending nonce was just matched");

        let fresh = now.saturating_duration_since(pending.issued_at) <= BUS_NONCE_TTL;
        let sender_matches = pending.name == sender;
        let host_matches = self
            .hosts
            .components
            .get(comp_name)
            .and_then(|r| r.active.as_ref().map(|(_, hid)| *hid))
            .and_then(|hid| self.hosts.hosts.get(&hid))
            .is_some_and(|host| match (&host.process, caller_proc) {
                (Some(host_proc), Some(caller_proc)) => host_proc.is(caller_proc),
                _ => false,
            });
        let duplicate = self.hosts.components.iter().any(|(&other, rec)| {
            other != comp_name && rec.accepted_bus_name.as_deref() == Some(sender)
        });

        if fresh && sender_matches && host_matches && !duplicate {
            if let Some(rec) = self.hosts.components.get_mut(comp_name) {
                rec.accepted_bus_name = Some(pending.name.clone());
            }
            pending
                .resource
                .bus_name(pending.name, BusNameResult::Accepted);
            Ok(())
        } else {
            pending
                .resource
                .bus_name(pending.name, BusNameResult::Refused);
            Err(Denial::Untrusted)
        }
    }

    /// Periodic host housekeeping: answer due backoffs, expire nonces and holds, answer quiet-moment restarts,
    /// create throttled bridges, and kill overdue expected-exit hosts.
    pub fn tick_hosts(&mut self, now: Instant) {
        // 1. Reset failure counters for components running stably for >= 60s.
        for rec in self.hosts.components.values_mut() {
            if rec.active.is_some()
                && rec
                    .started_at
                    .is_some_and(|s| now.saturating_duration_since(s) >= FAILURE_RESET_AFTER)
            {
                rec.consecutive_failures = 0;
                rec.failures.clear();
            }
        }

        // 2. Expire overdue bus nonces (5s).
        for rec in self.hosts.components.values_mut() {
            if rec
                .pending_nonce
                .as_ref()
                .is_some_and(|p| now.saturating_duration_since(p.issued_at) > BUS_NONCE_TTL)
            {
                if let Some(p) = rec.pending_nonce.take() {
                    p.resource.bus_name(p.name, BusNameResult::Refused);
                }
            }
        }

        // 3. Expire un-taken-over holds after 10s without `ready`, and fail timed-out restart waiters.
        let mut expired_hold_components = Vec::new();
        for (&name, rec) in &mut self.hosts.components {
            if rec.hold_expire_at.is_some_and(|at| now >= at) {
                rec.hold_expire_at = None;
                expired_hold_components.push(name);
            }
            let mut keep = Vec::new();
            for (deadline, cb) in std::mem::take(&mut rec.restart_waiters) {
                if now >= deadline {
                    cb(Err(format!("{name}: started and not ready")));
                } else {
                    keep.push((deadline, cb));
                }
            }
            rec.restart_waiters = keep;
        }
        for name in expired_hold_components {
            self.release_component_holds(name);
        }

        // 4. Answer due pending `connect` requests once backoff elapses.
        let pending_connects: Vec<(u64, &'static str)> = self
            .hosts
            .hosts
            .values()
            .flat_map(|h| h.connect_pending.iter().map(move |&c| (h.id, c)))
            .collect();
        for (hid, comp) in pending_connects {
            self.try_answer_component_connect(hid, comp, now);
        }

        // 5. Answer shared-host `request_restart` once quiet for 2s (and no grabs/transients) or 60s max.
        let idle_ok = now.saturating_duration_since(self.hosts.last_input_at) >= RESTART_QUIET_IDLE;
        let restart_ready: Vec<u64> = self
            .hosts
            .hosts
            .values()
            .filter_map(|h| {
                let req = h.restart_requested_at?;
                let max_hit = now.saturating_duration_since(req) >= RESTART_QUIET_MAX;
                let quiet = match h.kind {
                    HostKind::Separate(_) => true,
                    HostKind::Shared => idle_ok && !self.host_has_active_grab_or_transient(h.id),
                };
                (quiet || max_hit).then_some(h.id)
            })
            .collect();
        for hid in restart_ready {
            if let Some(host) = self.hosts.hosts.get_mut(&hid) {
                host.restart_requested_at = None;
                host.expected_exit_kill_at = Some(now + EXPECTED_EXIT_KILL_GRACE);
                if let Some(res) = &host.resource {
                    res.restart();
                }
            }
        }

        // 6. Kill expected-exit hosts past their 2s grace.
        for host in self.hosts.hosts.values_mut() {
            if host.expected_exit_kill_at.is_some_and(|at| now >= at) {
                host.expected_exit_kill_at = None;
                if let Some(proc) = &host.process {
                    if !proc.has_exited() {
                        tracing::warn!(
                            pid = proc.pid,
                            "killing shell host that did not exit in time"
                        );
                        let _ = proc.send(libc::SIGKILL);
                    }
                }
            }
        }

        // 7. Create throttled host-bus bridges whose retry time has arrived.
        let due_bridges: Vec<u64> = self
            .hosts
            .hosts
            .values()
            .filter_map(|h| {
                h.bridge_retry_at
                    .is_some_and(|at| now >= at)
                    .then_some(h.id)
            })
            .collect();
        for hid in due_bridges {
            if let Some(h) = self.hosts.hosts.get_mut(&hid) {
                h.bridge_retry_at = None;
            }
            self.create_bridge_for_separate_host(hid, now);
        }
    }

    /// True if any component running in `host_id` currently has a mapped transient layer surface or popup grab.
    fn host_has_active_grab_or_transient(&self, host_id: u64) -> bool {
        let host_clients: HashSet<ClientId> = self
            .hosts
            .components
            .values()
            .filter_map(|r| {
                r.active
                    .as_ref()
                    .filter(|(_, hid)| *hid == host_id)
                    .map(|(cid, _)| cid.clone())
            })
            .collect();
        if host_clients.is_empty() {
            return false;
        }
        // Check popup grabs.
        if let Some(ptr) = self.seat.get_pointer() {
            if ptr.is_grabbed() {
                return true;
            }
        }
        // Check mapped non-persisted (transient) layer surfaces belonging to this host's components.
        for o in &self.outputs {
            let map = layer_map_for_output(&o.output);
            for layer in map.layers() {
                let surf = layer.wl_surface();
                if let Some(client) = surf.client() {
                    if host_clients.contains(&client.id())
                        && !self.hosts.persisted.contains_key(&surf.id())
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Called from `CompositorHandler::commit` / `layer_committed` when a layer surface commits.
    /// Captures shm pixel snapshot for persisted surfaces and performs atomic hold takeover on first buffer commit.
    pub fn host_surface_committed(&mut self, surface: &WlSurface) {
        let surface_id = surface.id();
        let has_buffer = with_states(surface, |states| {
            states
                .data_map
                .get::<RendererSurfaceStateUserData>()
                .and_then(|d| d.lock().ok().map(|s| s.buffer().is_some()))
                .unwrap_or(false)
                || states
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .current()
                    .buffer
                    .is_some()
        });
        if !has_buffer {
            return;
        }
        let picture = HostManager::capture_shm_picture(surface);
        let takeover = {
            let Some(persisted) = self.hosts.persisted.get_mut(&surface_id) else {
                return;
            };
            if picture.is_some() {
                persisted.last_picture = picture;
            }
            if !persisted.mapped {
                persisted.mapped = true;
                Some((persisted.component, persisted.output, persisted.key.clone()))
            } else {
                None
            }
        };
        if let Some((component, output, key)) = takeover {
            let before = self.hosts.holds.len();
            self.hosts
                .holds
                .retain(|h| !(h.component == component && h.output == output && h.key == key));
            if self.hosts.holds.len() != before {
                tracing::debug!(component, %key, "held surface taken over atomically on first buffer commit");
                self.request_redraw();
            }
        }
    }

    /// Called from `WlrLayerShellHandler::layer_destroyed` BEFORE unmapping `layer` from `output`:
    /// if the surface was persisted and its component is not stopped, capture it into a [`HeldSurface`] so its
    /// reserved space and last picture stay until takeover or `ready`/timeout.
    pub fn host_layer_destroyed(
        &mut self,
        output: &smithay::output::Output,
        layer: &DesktopLayerSurface,
    ) {
        let surface = layer.wl_surface();
        let surface_id = surface.id();
        let Some(persisted) = self.hosts.persisted.remove(&surface_id) else {
            return;
        };
        let gave_up = self
            .hosts
            .components
            .get(persisted.component)
            .is_some_and(|r| r.gave_up);
        if gave_up {
            return;
        }
        let Some(output_id) = self.output_by_wl(output).map(|o| o.id) else {
            return;
        };
        let map = layer_map_for_output(output);
        let Some(geometry) = map.layer_geometry(layer) else {
            return;
        };
        let (wlr_layer, anchor, exclusive_zone, margin) = with_states(surface, |states| {
            let mut guard = states.cached_state.get::<LayerSurfaceCachedState>();
            let cur = guard.current();
            (cur.layer, cur.anchor, cur.exclusive_zone, cur.margin)
        });
        let picture = persisted
            .last_picture
            .or_else(|| HostManager::capture_shm_picture(surface));
        // Replace any older hold with the same (component, output, key).
        self.hosts.holds.retain(|h| {
            !(h.component == persisted.component && h.output == output_id && h.key == persisted.key)
        });
        self.hosts.holds.push(HeldSurface {
            component: persisted.component,
            key: persisted.key,
            output: output_id,
            layer: wlr_layer,
            geometry,
            anchor,
            exclusive_zone,
            margin,
            picture,
            held_since: Instant::now(),
        });
        if persisted.component == "windowmanager" {
            self.hosts.held_wm_outputs.insert(output_id);
        }
        self.request_redraw();
    }

    /// Release all holds owned by `component` and refresh usable areas on affected outputs.
    pub fn release_component_holds(&mut self, component: &'static str) {
        let mut outputs = HashSet::new();
        self.hosts.holds.retain(|h| {
            if h.component == component {
                outputs.insert(h.output);
                false
            } else {
                true
            }
        });
        if component == "windowmanager" {
            for &out in &outputs {
                self.hosts.held_wm_outputs.remove(&out);
            }
        }
        for out in outputs {
            self.refresh_usable_area(out);
        }
        self.request_redraw();
    }

    /// Dismiss all non-persisted (transient) layer surfaces belonging to `client_id`, release any grabs it holds,
    /// and restore keyboard focus to the key window if `client_id` held focus.
    pub fn dismiss_component_transients_and_grabs(&mut self, client_id: &ClientId) {
        // Release pointer/keyboard grabs if held by this client (popup grab or similar).
        let pointer_on_client = self
            .seat
            .get_pointer()
            .and_then(|p| p.current_focus())
            .and_then(|s| s.client())
            .is_some_and(|c| c.id() == *client_id);
        if pointer_on_client {
            if let Some(ptr) = self.seat.get_pointer() {
                ptr.unset_grab(self, smithay::utils::SERIAL_COUNTER.next_serial(), 0);
            }
        }
        let kbd_on_client = self
            .seat
            .get_keyboard()
            .and_then(|k| k.current_focus())
            .and_then(|s| s.client())
            .is_some_and(|c| c.id() == *client_id);
        if kbd_on_client {
            if let Some(kbd) = self.seat.get_keyboard() {
                kbd.unset_grab(self);
            }
        }

        // Dismiss all transient (non-persisted) layer surfaces of `client_id`.
        let mut to_dismiss: Vec<(OutputId, smithay::output::Output, DesktopLayerSurface)> =
            Vec::new();
        for o in &self.outputs {
            let map = layer_map_for_output(&o.output);
            for layer in map.layers() {
                let surf = layer.wl_surface();
                if surf.client().is_some_and(|c| c.id() == *client_id)
                    && !self.hosts.persisted.contains_key(&surf.id())
                {
                    to_dismiss.push((o.id, o.output.clone(), layer.clone()));
                }
            }
        }
        let mut touched_outputs = HashSet::new();
        for (oid, output, layer) in to_dismiss {
            layer_map_for_output(&output).unmap_layer(&layer);
            self.layer_keyboard_exclusivity(layer.wl_surface(), false);
            layer.layer_surface().send_close();
            touched_outputs.insert(oid);
        }
        for oid in touched_outputs {
            self.refresh_usable_area(oid);
        }
        if kbd_on_client {
            self.restore_keyboard_focus();
        }
        self.refocus_pointer();
        self.request_redraw();
    }

    /// Clean up host or component state when `client_id` disconnects.
    pub fn host_client_disconnected(&mut self, client_id: &ClientId) {
        let now = Instant::now();

        // Case 1: Host connection ended.
        if let Some(host_id) = self.hosts.host_by_client.remove(client_id) {
            if let Some(host) = self.hosts.hosts.remove(&host_id) {
                let expected = host.expected_exit_kill_at.is_some();
                match host.kind {
                    HostKind::Separate(comp) => {
                        if let Some(shared) = self.shared_host_resource() {
                            shared.bus_peer_gone(comp.to_string());
                        }
                    }
                    HostKind::Shared => {
                        if !expected {
                            self.hosts.shared_host_crashes.retain(|t| {
                                now.saturating_duration_since(*t) <= SHARED_HOST_CRASH_WINDOW
                            });
                            self.hosts.shared_host_crashes.push(now);
                            if self.hosts.shared_host_crashes.len()
                                >= SHARED_HOST_CRASH_ISOLATE_COUNT
                            {
                                tracing::warn!(
                                    "shared shell host crashed {} times within 60s: isolating components in separate hosts for this session",
                                    self.hosts.shared_host_crashes.len()
                                );
                                self.hosts.all_separate_for_session = true;
                            }
                        }
                    }
                }
            }
            return;
        }

        // Case 2: Component connection ended.
        let comp_name = self.hosts.components.iter().find_map(|(&name, rec)| {
            rec.active
                .as_ref()
                .is_some_and(|(cid, _)| cid == client_id)
                .then_some(name)
        });
        if let Some(name) = comp_name {
            self.dismiss_component_transients_and_grabs(client_id);
            if let Some(rec) = self.hosts.components.get_mut(name) {
                rec.active = None;
                rec.component_resource = None;
                rec.liveness = Liveness::Responsive;
                rec.accepted_bus_name = None;
                rec.pending_nonce = None;
                rec.ready = false;
            }
        }
    }

    fn shared_host_resource(&self) -> Option<CupertinoHostV1> {
        self.hosts
            .hosts
            .values()
            .find(|h| h.kind == HostKind::Shared)
            .and_then(|h| h.resource.clone())
    }

    fn create_bridge_for_separate_host(&mut self, separate_host_id: u64, now: Instant) {
        let Some(shared_res) = self.shared_host_resource() else {
            return;
        };
        let Some(sep) = self.hosts.hosts.get_mut(&separate_host_id) else {
            return;
        };
        let HostKind::Separate(comp) = sep.kind else {
            return;
        };
        let Some(sep_res) = sep.resource.clone() else {
            return;
        };
        if let Some(last) = sep.last_bridge_at {
            let elapsed = now.saturating_duration_since(last);
            if elapsed < BRIDGE_THROTTLE {
                sep.bridge_retry_at = Some(last + BRIDGE_THROTTLE);
                return;
            }
        }
        match bridge_socketpair() {
            Ok((shared_fd, sep_fd)) => {
                sep.last_bridge_at = Some(now);
                shared_res.bus_peer(comp.to_string(), shared_fd.as_fd());
                sep_res.bus(sep_fd.as_fd());
            }
            Err(e) => {
                tracing::warn!(component = comp, error = %e, "creating host bus bridge socketpair failed");
            }
        }
    }

    fn record_component_failure(&mut self, component: &'static str, lost: bool, now: Instant) {
        let mut gave_up_now = false;
        if let Some(rec) = self.hosts.components.get_mut(component) {
            // Loss window rule (D16): exclude the run duration of an instance lost within 60s of start so 5 consecutive
            // startup hangs still trigger give-up within the rolling 60s window.
            let stamp = if lost {
                match rec.started_at {
                    Some(started) if now.saturating_duration_since(started) < GIVE_UP_WINDOW => {
                        started
                    }
                    _ => now,
                }
            } else {
                now
            };
            rec.failures
                .retain(|t| stamp.saturating_duration_since(*t) <= GIVE_UP_WINDOW);
            rec.failures.push(stamp);
            rec.consecutive_failures = rec.consecutive_failures.saturating_add(1);
            if rec.failures.len() >= GIVE_UP_FAILURES {
                rec.gave_up = true;
                rec.backoff_until = None;
                gave_up_now = true;
            } else {
                rec.backoff_until = Some(now + backoff_for(rec.consecutive_failures));
            }
            if lost {
                rec.separate_for_session = true;
            }
        }
        if gave_up_now {
            tracing::warn!(component, "component hit 5 failures within 60s: giving up");
            self.release_component_holds(component);
            if let Some(shared) = self.shared_host_resource() {
                shared.component_stopped(component.to_string());
            }
        }
    }

    fn try_answer_component_connect(
        &mut self,
        host_id: u64,
        component: &'static str,
        now: Instant,
    ) {
        let (in_list, obj, host_holder) = {
            let Some(host) = self.hosts.hosts.get(&host_id) else {
                return;
            };
            if !host.connect_pending.contains(component) {
                return;
            }
            let in_list = host.components.contains(&component);
            let obj = host.component_objects.get(component).cloned();
            let holder = match &host.process {
                Some(p) => p.try_clone().ok().map(Holder::Process),
                None => Some(Holder::Detached(
                    now + crate::authorization::DETACHED_LIFETIME,
                )),
            };
            (in_list, obj, holder)
        };
        if !in_list {
            return;
        }
        let Some(obj) = obj else {
            return;
        };
        let (gave_up, backoff_until) = match self.hosts.components.get(component) {
            Some(rec) => (rec.gave_up, rec.backoff_until),
            None => return,
        };
        if gave_up {
            if let Some(host) = self.hosts.hosts.get_mut(&host_id) {
                host.connect_pending.remove(component);
            }
            obj.stopped(StopReason::GaveUp);
            return;
        }
        if backoff_until.is_some_and(|at| now < at) {
            return;
        }

        // Close any prior active connection for this component so roles live in one host at a time.
        let prior_client = self
            .hosts
            .components
            .get(component)
            .and_then(|r| r.active.as_ref().map(|(cid, _)| cid.clone()));
        if let Some(old_cid) = prior_client {
            self.display_handle.backend_handle().kill_client(
                old_cid.clone(),
                smithay::reexports::wayland_server::backend::DisconnectReason::ConnectionClosed,
            );
            self.host_client_disconnected(&old_cid);
        }

        let Some(spec) = component_spec(component) else {
            return;
        };
        let Ok((server_end, client_end)) = UnixStream::pair() else {
            tracing::warn!(component, "creating component Wayland socketpair failed");
            return;
        };
        let instance = {
            let rec = self
                .hosts
                .components
                .get_mut(component)
                .expect("component exists in table");
            rec.instance = rec.instance.saturating_add(1);
            rec.instance
        };
        let state = ClientState {
            capabilities: ClientCapabilitySet::of(spec.roles.iter().copied()),
            component: Some(ComponentConnection {
                name: component,
                instance,
                host_id,
            }),
            ..ClientState::default()
        };
        if let Some(holder) = host_holder {
            let _ = state.holder.set(holder);
        }
        let _ = state.gone.set(self.gone_clients.clone());
        state.look_up_process();
        let Ok(client) =
            crate::state::insert_client_handle(&self.display_handle, server_end, state)
        else {
            tracing::warn!(component, "inserting component Wayland client failed");
            return;
        };
        let client_id = client.id();
        if let Some(host) = self.hosts.hosts.get_mut(&host_id) {
            host.connect_pending.remove(component);
        }
        if let Some(rec) = self.hosts.components.get_mut(component) {
            rec.active = Some((client_id, host_id));
            rec.started_at = Some(now);
            rec.liveness = Liveness::Responsive;
            rec.ready = false;
            rec.hold_expire_at = Some(now + HOLD_READY_TIMEOUT);
        }
        let client_fd = OwnedFd::from(client_end);
        obj.connection(client_fd.as_fd());
    }
}

impl GlobalDispatch<CupertinoHostV1, ()> for Cupertino {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<CupertinoHostV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let Some(&host_id) = state.hosts.host_by_client.get(&client.id()) else {
            return;
        };
        let host_obj = init.init(resource, HostGlobalData { host_id });
        let (kind, components) = {
            let Some(host) = state.hosts.hosts.get_mut(&host_id) else {
                return;
            };
            host.resource = Some(host_obj.clone());
            (host.kind, host.components.clone())
        };
        host_obj.kind(match kind {
            HostKind::Shared => WireHostKind::Shared,
            HostKind::Separate(_) => WireHostKind::Separate,
        });
        for name in components {
            host_obj.component(name.to_string());
        }
        host_obj.done();

        let now = Instant::now();
        match kind {
            HostKind::Separate(_) => state.create_bridge_for_separate_host(host_id, now),
            HostKind::Shared => {
                let separate_ids: Vec<u64> = state
                    .hosts
                    .hosts
                    .values()
                    .filter(|h| matches!(h.kind, HostKind::Separate(_)) && h.resource.is_some())
                    .map(|h| h.id)
                    .collect();
                for sid in separate_ids {
                    state.create_bridge_for_separate_host(sid, now);
                }
            }
        }
    }

    fn can_view(client: Client, _: &()) -> bool {
        authorized(&client, ShellRole::Host)
    }
}

impl Dispatch<CupertinoHostV1, HostGlobalData> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &CupertinoHostV1,
        request: cupertino_host_v1::Request,
        data: &HostGlobalData,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        let now = Instant::now();
        match request {
            cupertino_host_v1::Request::Destroy => {
                if let Some(host) = state.hosts.hosts.get_mut(&data.host_id) {
                    host.resource = None;
                }
            }
            cupertino_host_v1::Request::GetComponent { id, name } => {
                let Some(spec) = component_spec(&name) else {
                    resource.post_error(HostError::Name, format!("unknown component name: {name}"));
                    return;
                };
                let already = state
                    .hosts
                    .hosts
                    .get(&data.host_id)
                    .is_some_and(|h| h.component_objects.contains_key(spec.name));
                if already {
                    resource.post_error(
                        HostError::Component,
                        format!("host already holds an object for {}", spec.name),
                    );
                    return;
                }
                let obj = init.init(
                    id,
                    HostComponentData {
                        host_id: data.host_id,
                        component: spec.name,
                    },
                );
                if let Some(host) = state.hosts.hosts.get_mut(&data.host_id) {
                    host.component_objects.insert(spec.name, obj);
                }
            }
            cupertino_host_v1::Request::RequestRestart => {
                let Some(host) = state.hosts.hosts.get_mut(&data.host_id) else {
                    return;
                };
                match host.kind {
                    HostKind::Separate(_) => {
                        host.expected_exit_kill_at = Some(now + EXPECTED_EXIT_KILL_GRACE);
                        resource.restart();
                    }
                    HostKind::Shared => {
                        if host.restart_requested_at.is_none() {
                            host.restart_requested_at = Some(now);
                        }
                    }
                }
            }
            cupertino_host_v1::Request::BusDropped { component } => {
                let Some(spec) = component_spec(&component) else {
                    resource.post_error(
                        HostError::Name,
                        format!("unknown component name: {component}"),
                    );
                    return;
                };
                let Some(host) = state.hosts.hosts.get(&data.host_id) else {
                    return;
                };
                match host.kind {
                    HostKind::Separate(own) if own != spec.name => {
                        resource.post_error(
                            HostError::Kind,
                            format!(
                                "separate host for {own} cannot report bus_dropped for {}",
                                spec.name
                            ),
                        );
                    }
                    HostKind::Separate(_) => {
                        state.create_bridge_for_separate_host(data.host_id, now);
                    }
                    HostKind::Shared => {
                        let sep_id = state
                            .hosts
                            .hosts
                            .values()
                            .find(|h| h.kind == HostKind::Separate(spec.name))
                            .map(|h| h.id);
                        if let Some(sid) = sep_id {
                            state.create_bridge_for_separate_host(sid, now);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<CupertinoHostComponentV1, HostComponentData> for Cupertino {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &CupertinoHostComponentV1,
        request: cupertino_host_component_v1::Request,
        data: &HostComponentData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let now = Instant::now();
        let component = data.component;
        match request {
            cupertino_host_component_v1::Request::Destroy => {
                if let Some(host) = state.hosts.hosts.get_mut(&data.host_id) {
                    host.component_objects.remove(component);
                    host.connect_pending.remove(component);
                }
                let active_here = state.hosts.components.get(component).and_then(|r| {
                    r.active
                        .as_ref()
                        .filter(|(_, hid)| *hid == data.host_id)
                        .map(|(cid, _)| cid.clone())
                });
                if let Some(cid) = active_here {
                    state.display_handle.backend_handle().kill_client(
                        cid.clone(),
                        smithay::reexports::wayland_server::backend::DisconnectReason::ConnectionClosed,
                    );
                    state.host_client_disconnected(&cid);
                }
            }
            cupertino_host_component_v1::Request::Connect => {
                // Close any existing connection for this component in this host first.
                let active_here = state.hosts.components.get(component).and_then(|r| {
                    r.active
                        .as_ref()
                        .filter(|(_, hid)| *hid == data.host_id)
                        .map(|(cid, _)| cid.clone())
                });
                if let Some(cid) = active_here {
                    state.display_handle.backend_handle().kill_client(
                        cid.clone(),
                        smithay::reexports::wayland_server::backend::DisconnectReason::ConnectionClosed,
                    );
                    state.host_client_disconnected(&cid);
                }
                if let Some(host) = state.hosts.hosts.get_mut(&data.host_id) {
                    if !host.connect_pending.insert(component) {
                        // Already outstanding: idempotent.
                        return;
                    }
                }
                state.try_answer_component_connect(data.host_id, component, now);
            }
            cupertino_host_component_v1::Request::Busy => {
                if let Some(rec) = state.hosts.components.get_mut(component) {
                    if rec
                        .active
                        .as_ref()
                        .is_some_and(|(_, hid)| *hid == data.host_id)
                    {
                        rec.liveness = Liveness::Busy;
                    }
                }
            }
            cupertino_host_component_v1::Request::Unresponsive => {
                let active_cid = state.hosts.components.get_mut(component).and_then(|rec| {
                    if rec
                        .active
                        .as_ref()
                        .is_some_and(|(_, hid)| *hid == data.host_id)
                    {
                        rec.liveness = Liveness::Unresponsive;
                        rec.active.as_ref().map(|(cid, _)| cid.clone())
                    } else {
                        None
                    }
                });
                if let Some(cid) = active_cid {
                    state.dismiss_component_transients_and_grabs(&cid);
                }
            }
            cupertino_host_component_v1::Request::Responsive => {
                if let Some(rec) = state.hosts.components.get_mut(component) {
                    if rec
                        .active
                        .as_ref()
                        .is_some_and(|(_, hid)| *hid == data.host_id)
                    {
                        rec.liveness = Liveness::Responsive;
                    }
                }
                state.refocus_pointer();
            }
            cupertino_host_component_v1::Request::Lost => {
                if let Some(rec) = state.hosts.components.get_mut(component) {
                    rec.liveness = Liveness::Lost;
                }
                state.record_component_failure(component, true, now);
                let kind = state.hosts.hosts.get(&data.host_id).map(|h| h.kind);
                match kind {
                    Some(HostKind::Shared) => {
                        let new_list: Vec<&'static str> = state
                            .hosts
                            .hosts
                            .get(&data.host_id)
                            .map(|h| {
                                h.components
                                    .iter()
                                    .copied()
                                    .filter(|&c| c != component)
                                    .collect()
                            })
                            .unwrap_or_default();
                        state.set_host_components(data.host_id, new_list);
                        if !state.hosts.component_gave_up(component) {
                            state.hosts.requested_separate_hosts.push(component);
                        }
                    }
                    Some(HostKind::Separate(_)) => {
                        if let Some(host) = state.hosts.hosts.get_mut(&data.host_id) {
                            host.expected_exit_kill_at = Some(now + EXPECTED_EXIT_KILL_GRACE);
                        }
                        if !state.hosts.component_gave_up(component) {
                            state.hosts.requested_separate_hosts.push(component);
                        }
                    }
                    None => {}
                }
            }
            cupertino_host_component_v1::Request::Failed { cause } => {
                let WEnum::Value(cause) = cause else {
                    resource.post_error(HostComponentError::Cause, "invalid failure cause");
                    return;
                };
                tracing::warn!(component, ?cause, "shell component failed");
                state.record_component_failure(component, false, now);
            }
            cupertino_host_component_v1::Request::Unavailable { reason } => {
                let WEnum::Value(reason) = reason else {
                    resource.post_error(HostComponentError::Reason, "invalid unavailable reason");
                    return;
                };
                tracing::warn!(component, ?reason, "shell component unavailable in host");
                if let Some(host) = state.hosts.hosts.get_mut(&data.host_id) {
                    host.connect_pending.remove(component);
                }
            }
            _ => {}
        }
    }
}

impl GlobalDispatch<CupertinoComponentV1, ()> for Cupertino {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<CupertinoComponentV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let Some(conn) = client.get_data::<ClientState>().and_then(|s| s.component) else {
            return;
        };
        let obj = init.init(
            resource,
            ComponentGlobalData {
                component: conn.name,
                instance: conn.instance,
            },
        );
        if let Some(rec) = state.hosts.components.get_mut(conn.name) {
            rec.component_resource = Some(obj.clone());
        }
        obj.component(conn.name.to_string(), conn.instance);
        let keys: Vec<String> = state
            .hosts
            .holds
            .iter()
            .filter(|h| h.component == conn.name)
            .map(|h| h.key.clone())
            .collect();
        for key in keys {
            obj.hold(key);
        }
        obj.done();
    }

    fn can_view(client: Client, _: &()) -> bool {
        client
            .get_data::<ClientState>()
            .is_some_and(|s| s.component.is_some())
    }
}

impl Dispatch<CupertinoComponentV1, ComponentGlobalData> for Cupertino {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoComponentV1,
        request: cupertino_component_v1::Request,
        data: &ComponentGlobalData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let now = Instant::now();
        match request {
            cupertino_component_v1::Request::Destroy => {
                if let Some(rec) = state.hosts.components.get_mut(data.component) {
                    if rec
                        .component_resource
                        .as_ref()
                        .is_some_and(|r| r == resource)
                    {
                        rec.component_resource = None;
                    }
                }
            }
            cupertino_component_v1::Request::AnnounceBusName { name } => {
                if !is_unique_bus_name(&name) {
                    resource.post_error(
                        ComponentError::Name,
                        format!("not a valid unique bus name: {name}"),
                    );
                    return;
                }
                let nonce = generate_nonce();
                if let Some(rec) = state.hosts.components.get_mut(data.component) {
                    // Re-announcing immediately revokes any prior accepted bus name.
                    rec.accepted_bus_name = None;
                    rec.pending_nonce = Some(PendingBusNonce {
                        name,
                        nonce: nonce.clone(),
                        issued_at: now,
                        resource: resource.clone(),
                    });
                }
                resource.bus_nonce(nonce);
            }
            cupertino_component_v1::Request::Persist { surface, key } => {
                if !surface.is_alive() || surface.client().as_ref() != Some(client) {
                    resource.post_error(
                        ComponentError::InvalidSurface,
                        "surface does not belong to this client or is gone",
                    );
                    return;
                }
                if get_role(&surface) != Some(LAYER_SURFACE_ROLE) {
                    resource.post_error(
                        ComponentError::InvalidSurface,
                        "surface is not a layer shell surface",
                    );
                    return;
                }
                let already_mapped = with_states(&surface, |states| {
                    states
                        .data_map
                        .get::<RendererSurfaceStateUserData>()
                        .and_then(|d| d.lock().ok().map(|s| s.buffer().is_some()))
                        .unwrap_or(false)
                        || states
                            .cached_state
                            .get::<SurfaceAttributes>()
                            .current()
                            .buffer
                            .is_some()
                });
                if already_mapped {
                    resource.post_error(
                        ComponentError::Mapped,
                        "persist must be sent before the surface commits its first buffer",
                    );
                    return;
                }
                if key.is_empty() || key.len() > MAX_PERSIST_KEY_BYTES {
                    resource.post_error(ComponentError::Key, "persist key must be 1..=64 bytes");
                    return;
                }
                // Find the output this layer surface is mapped on.
                let output_id = state.outputs.iter().find_map(|o| {
                    let map = layer_map_for_output(&o.output);
                    let found = map.layers().any(|l| l.wl_surface() == &surface);
                    found.then_some(o.id)
                });
                let Some(output_id) = output_id else {
                    resource.post_error(
                        ComponentError::InvalidSurface,
                        "layer surface is not mapped on any output",
                    );
                    return;
                };
                let client_id = client.id();
                let dup = state.hosts.persisted.values().any(|p| {
                    p.client_id == client_id
                        && p.output == output_id
                        && p.key == key
                        && p.surface_id != surface.id()
                });
                if dup {
                    resource.post_error(
                        ComponentError::Key,
                        format!("duplicate persist key `{key}` on the same output"),
                    );
                    return;
                }
                state.hosts.persisted.insert(
                    surface.id(),
                    PersistedSurface {
                        component: data.component,
                        client_id,
                        surface_id: surface.id(),
                        key,
                        output: output_id,
                        mapped: false,
                        last_picture: None,
                    },
                );
            }
            cupertino_component_v1::Request::Ready => {
                if let Some(rec) = state.hosts.components.get_mut(data.component) {
                    if rec.instance == data.instance {
                        rec.ready = true;
                        rec.hold_expire_at = None;
                        for (_, cb) in std::mem::take(&mut rec.restart_waiters) {
                            cb(Ok(()));
                        }
                    }
                }
                state.release_component_holds(data.component);
            }
            _ => {}
        }
    }
}

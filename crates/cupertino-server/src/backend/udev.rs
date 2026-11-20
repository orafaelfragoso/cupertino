//! TTY backend: logind session, sysfs device discovery, every DRM card with its KMS outputs (one head per connected
//! connector, see `crate::displays`), evdev input devices (see `input.rs`). One card renders every frame (the render
//! GPU); a display on another card shows a copy of it that its own card draws.
//!
//! The render loop is driven by the main loop: an output is rendered when the compositor wants
//! frames and no frame is waiting for its vblank; the vblank event reports presentation.
//!
//! NOTE: written against the Smithay 0.7 API and compile-checked; it cannot run while another
//! session holds the DRM master (for example when started from a running desktop session).

use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use cupertino_devices::{Action, DevT, DeviceEvent, Monitor, Subsystem, SEAT0};
use cupertino_scene::renderer::{
    Completion as _, Failure, RenderFailure as _, RenderStatus, Renderer,
};
use cupertino_server_backend::session::LogindSession;
use cupertino_wm::OutputId;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::drm::compositor::{FrameError, FrameFlags, RenderFrameError};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{
    DrmOutputManager, DrmOutputManagerError, DrmOutputRenderElements,
};
use smithay::backend::drm::{
    DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmError, DrmEvent, DrmEventTime, DrmNode, NodeType,
};
use smithay::backend::egl::context::ContextPriority;
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::renderer::utils::Buffer;
use smithay::backend::renderer::Color32F;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{
    EventLoop, Interest, LoopHandle, Mode, PostAction, RegistrationToken,
};
use smithay::reexports::drm::control::{
    self, connector, crtc, Device as ControlDevice, ModeFlags, ModeTypeFlags,
};
use smithay::reexports::drm::Device as DrmDeviceTrait;
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::utils::{DeviceFd, Transform};
use smithay::wayland::drm_syncobj::{supports_syncobj_eventfd, DrmSyncobjState};
use smithay::wayland::presentation::Refresh;
use smithay_drm_extras::drm_scanner::DrmScanner;

use super::copy::{self, FrameCopy, Unsupported, Via};
use super::input;
use super::Backend;
use crate::displays::{edid, HeadCaps, HeadId, HeadInfo, HeadMode, HeadOp};
use crate::gpu_reset::Loss;
use crate::latch::{FrameTimes, LateLatch};
use crate::output_depth::Depth;
use crate::render::gles::{Gles, GlesCompletion, GlesResources, GlesTarget, RenderError};
use crate::render::scene::RenderResources;
use crate::schedule::Overdue;
use crate::software_render;
use crate::state::Cupertino;
use crate::sync_file::RenderFence;
pub use cupertino_server_backend::backend::{DrmSurfaceOutput, Frame};

const CLEAR: Color32F = Color32F::new(0.05, 0.05, 0.07, 1.0);

type Manager = DrmOutputManager<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    Frame,
    DrmDeviceFd,
>;

struct Surface {
    output: OutputId,
    drm_output: DrmSurfaceOutput,
    /// On a card that does not render: the copies of the render GPU's frames it shows.
    copy: Option<FrameCopy<DrmDeviceFd>>,
    /// The client buffers the copy's late frame was drawn from, kept until it is shown ([`copy::Brought::Late`]).
    late_buffers: Vec<Buffer>,
    /// A frame was queued and its vblank has not arrived yet.
    pending: bool,
    refresh: Duration,
    /// The recent render times, and how late after a vblank the next frame may start ([`crate::latch`]).
    latch: LateLatch,
    /// The vblank the frame in flight was drawn for, where the vblank phase was known when it started.
    target: Option<Duration>,
    /// The times of the frame in flight, read into the latch when its flip comes.
    in_flight: Option<InFlight>,
}

/// A queued frame's start, CPU time and render fence, kept until its flip ([`crate::latch::FrameTimes`]).
struct InFlight {
    /// On the monotonic clock of the flips' timestamps.
    started: Duration,
    cpu: Duration,
    fence: RenderFence,
    held: bool,
}

/// A display enabled while the session waits in the background to be shown ([`Preparing`]): its output is announced to
/// clients, which draw on it, and its DRM output is set up when the session is shown, since setting one up needs the
/// device's master, which only the shown session holds.
struct Prepared {
    head: HeadId,
    output: OutputId,
    /// What the output is announced with, for its DRM output.
    announced: smithay::output::Output,
    mode: HeadMode,
    drm_mode: control::Mode,
    connector: connector::Handle,
}

/// The session started on its terminal in the background ([`run`]): before the console comes to it, it opens the GPU,
/// announces its displays' outputs, starts its shell and draws its first frame once off screen (the wallpaper read, the
/// programs and textures made), then tells the session manager it can be shown ([`tell_prepared`]).
struct Preparing {
    since: Instant,
    /// The session manager was told.
    told: bool,
}

/// A connected display: its connector and the DRM modes behind its head's modes.
struct DrmHead {
    head: HeadId,
    connector: connector::Handle,
    /// The display's EDID when it connected: another one means another display on the connector.
    edid: Option<Vec<u8>>,
    modes: Vec<(HeadMode, control::Mode)>,
}

impl DrmHead {
    fn drm_mode(&self, mode: HeadMode) -> Result<control::Mode> {
        self.modes
            .iter()
            .find(|(m, _)| *m == mode)
            .map(|(_, d)| *d)
            .ok_or_else(|| {
                anyhow!(
                    "no DRM mode {}x{}@{}",
                    mode.width,
                    mode.height,
                    mode.refresh_mhz
                )
            })
    }
}

/// A DRM card and the displays on it.
struct Card {
    /// The device number its uevents carry.
    devnum: DevT,
    /// What its connectors' names start with: nothing on the primary card, `card1-` on another one, so two cards' `DP-1`
    /// are told apart.
    prefix: String,
    /// The render GPU's renderer draws every frame; another card's only draws the copy of one onto its display.
    renderer: GlesRenderer,
    /// Allocates the render GPU's buffers for the displays of other cards.
    gbm: GbmDevice<DrmDeviceFd>,
    manager: Manager,
    scanner: DrmScanner,
    /// The enabled heads' outputs.
    surfaces: HashMap<crtc::Handle, Surface>,
    /// The heads enabled while the session waited to be shown, by CRTC, set up when it is.
    prepared: HashMap<crtc::Handle, Prepared>,
    /// Every connected head, by the CRTC the scanner gave its connector.
    heads: HashMap<crtc::Handle, DrmHead>,
    /// Connected connectors the scanner found no free CRTC for, told once.
    waiting: HashSet<connector::Handle>,
    /// Its vblank source, removed with the card.
    events: RegistrationToken,
}

pub struct UdevBackend {
    session: LogindSession,
    /// The card that renders. It stays while the session runs.
    render_node: DrmNode,
    cards: HashMap<DrmNode, Card>,
    /// The command line's scale for every display.
    scale: Option<f64>,
    /// The session is paused: the DRM device rejects every frame until it resumes.
    paused: bool,
    /// The session has not been shown yet: it gets ready in the background.
    preparing: Option<Preparing>,
    /// The display sleeps (`power.rs`): every CRTC is off and nothing is rendered until it wakes.
    asleep: bool,
    /// A render reported the rendering context lost: nothing is drawn with it again, and the next frame makes a new one
    /// ([`recover_gpu`]) or ends the session.
    renderer_lost: bool,
    /// The depth of the primary planes of every card, from the configuration at the start.
    depth: Depth,
    /// Frames start late after a vblank with a real timestamp ([`crate::latch`]); off by `CUPERTINO_LATE_LATCH=0`.
    late_latch: bool,
}

/// Whether an output gets a frame now. A paused session or a lost renderer renders nothing: the device rejects the
/// frame, and the loop would retry (and log) every iteration. Nor does an output whose frame waits for its vblank, or
/// one its scheduler does not find due (a change at once, animation on its refresh grid, a failed frame's retry).
fn frame_due(stopped: bool, waiting: bool, due: bool) -> bool {
    !stopped && !waiting && due
}

/// How long the loop may sleep: until the earliest output is due to draw, a flip in flight is overdue,
/// [`Cupertino::tick`] has time-driven work ([`Cupertino::next_tick_deadline`]) or the run's limit (`end`) comes.
/// Events (clients, input, vblanks) wake it sooner; with nothing due it sleeps ([`super::REST_WAIT`]). Outputs that cannot
/// draw (paused, asleep, off) wait for an event, not a frame deadline; a session getting ready in the background looks
/// every [`PREPARE_POLL`] whether its first frame can be made.
fn loop_timeout(state: &Cupertino, end: Option<Instant>) -> Duration {
    // a passed deadline nothing acted on must not spin the loop
    const PASSED: Duration = Duration::from_millis(1);
    let tick = super::tick_wait(state);
    let frames = match state.backend.as_ref() {
        Some(Backend::Udev(u)) if u.renderer_lost && !(u.paused || u.asleep) => Some(PASSED),
        Some(Backend::Udev(u)) if !(u.paused || u.asleep || u.renderer_lost) => {
            let now = state.frame_now();
            u.cards
                .values()
                .flat_map(|card| card.surfaces.values())
                .filter(|s| state.displays.powered(s.output))
                .filter_map(|s| {
                    let idx = state.outputs.iter().position(|o| o.id == s.output)?;
                    if s.pending {
                        state.outputs[idx].scheduler.overdue_deadline(s.refresh)
                    } else {
                        state.frame_deadline(idx, now)
                    }
                })
                .min()
                .map(|due| if due < now { PASSED } else { due - now })
        }
        _ => None,
    };
    let end = end.map(|end| end.saturating_duration_since(Instant::now()));
    // a session in the background looks for its wallpaper until it can be shown
    let preparing = match state.backend.as_ref() {
        Some(Backend::Udev(u)) if u.preparing.as_ref().is_some_and(|p| !p.told) => {
            Some(PREPARE_POLL)
        }
        _ => None,
    };
    super::loop_wait([tick, frames, end, preparing])
}

impl UdevBackend {
    pub fn renderer(&mut self) -> &mut GlesRenderer {
        &mut self
            .cards
            .get_mut(&self.render_node)
            .expect("the render GPU's card stays")
            .renderer
    }

    /// The card of the uevents that carry `devnum`.
    fn card_of(&self, devnum: DevT) -> Option<DrmNode> {
        self.cards
            .iter()
            .find(|(_, card)| card.devnum == devnum)
            .map(|(node, _)| *node)
    }

    /// The card and CRTC that drive `head`.
    fn head_place(&self, head: HeadId) -> Option<(DrmNode, crtc::Handle)> {
        self.cards.iter().find_map(|(node, card)| {
            card.heads
                .iter()
                .find(|(_, h)| h.head == head)
                .map(|(crtc, _)| (*node, *crtc))
        })
    }

    pub fn renderer_lost(&self) -> bool {
        self.renderer_lost
    }

    /// Whether no frame reaches the screens: the session is paused or every CRTC is off.
    pub fn shows_nothing(&self) -> bool {
        self.paused || self.asleep
    }

    /// Switch every CRTC off (DPMS off) or let the next frame switch it back on with a full redraw.
    pub fn set_asleep(&mut self, asleep: bool) {
        if self.asleep == asleep {
            return;
        }
        self.asleep = asleep;
        for s in self
            .cards
            .values_mut()
            .flat_map(|c| c.surfaces.values_mut())
        {
            if asleep {
                // a frame waiting for its vblank is dropped with the cleared state
                s.pending = false;
                if let Err(e) = s.drm_output.with_compositor(|c| c.clear()) {
                    tracing::warn!(error = %e, "switching the output off failed");
                }
            } else {
                s.drm_output.reset_buffers();
            }
        }
    }

    pub fn switch_vt(&mut self, vt: i32) {
        if let Err(e) = self.session.change_vt(vt) {
            tracing::warn!(vt, error = %e, "switching the virtual terminal failed");
        }
    }
}

fn udev(state: &mut Cupertino) -> Option<&mut UdevBackend> {
    match state.backend.as_mut() {
        Some(Backend::Udev(u)) => Some(u),
        _ => None,
    }
}

/// A card opened through the session.
struct Opened {
    drm: DrmDevice,
    notifier: DrmDeviceNotifier,
    fd: DrmDeviceFd,
    gbm: GbmDevice<DrmDeviceFd>,
    renderer: GlesRenderer,
    /// The card's render node, for the exporter's direct scan-out of client buffers.
    render_node: Option<DrmNode>,
    /// Its only EGL device is a software one.
    software: bool,
}

fn open_card(session: &LogindSession, path: &Path) -> Result<Opened> {
    let node =
        DrmNode::from_path(path).map_err(|e| anyhow!("drm node for {}: {e}", path.display()))?;
    let fd = session
        .clone()
        .open(
            path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        )
        .map_err(|e| anyhow!("opening {}: {e}", path.display()))?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    // the displays keep what they show (the login screen, the session before, the console) until a frame of ours replaces
    // it, rather than going dark when the device is opened: a CRTC in the mode it had changes its picture without a
    // modeset, and what no head of ours drives is switched off after each scan ([`switch_off_strays`]). Opening the device
    // so commits nothing, which a session in the background (no DRM master) could not
    let (drm, notifier) =
        DrmDevice::new(fd.clone(), false).map_err(|e| anyhow!("drm device: {e}"))?;
    let gbm = GbmDevice::new(fd.clone()).map_err(|e| anyhow!("gbm device: {e} (the driver's GBM backend is missing or refused the device; run `cupertino-server --check-hardware`)"))?;
    let display = unsafe { EGLDisplay::new(gbm.clone()) }.map_err(|e| {
        anyhow!(
            "EGL display: {e} (no EGL platform for GBM; check the driver's EGL and GBM libraries)"
        )
    })?;
    let egl_device =
        EGLDevice::device_for_display(&display).map_err(|e| anyhow!("EGL device: {e}"))?;
    let render_node = egl_device
        .try_get_render_node()
        .ok()
        .flatten()
        .or_else(|| node.node_with_type(NodeType::Render).and_then(|n| n.ok()));
    let context = EGLContext::new_with_reset_notification(&display, Some(ContextPriority::High))
        .map_err(|e| anyhow!("EGL context: {e}"))?;
    let renderer =
        unsafe { super::gles_renderer(context) }.map_err(|e| anyhow!("GLES renderer: {e}"))?;
    Ok(Opened {
        drm,
        notifier,
        fd,
        gbm,
        renderer,
        render_node,
        software: egl_device.is_software(),
    })
}

/// The GPUs other than `gpu` that can render, in their order.
fn others(
    gpus: &[cupertino_devices::Gpu],
    gpu: &cupertino_devices::Gpu,
) -> Vec<cupertino_devices::Gpu> {
    gpus.iter()
        .filter(|g| g.device != gpu.device && g.render_node.is_some())
        .cloned()
        .collect()
}

/// The connector-name prefix of `gpu`'s displays: none on the primary card, its name on another one.
fn card_prefix(gpu: &cupertino_devices::Gpu, primary: &cupertino_devices::Gpu) -> String {
    if gpu.device == primary.device {
        String::new()
    } else {
        format!("{}-", gpu.device.sysname())
    }
}

/// Set up the displays of an opened card and listen for its vblanks. `renders`: it is the render GPU, whose exporter
/// may scan client buffers out directly.
fn card(
    handle: &LoopHandle<'static, Cupertino>,
    node: DrmNode,
    devnum: DevT,
    prefix: String,
    opened: Opened,
    renders: bool,
    depth: Depth,
) -> Result<Card> {
    let render_formats = opened
        .renderer
        .egl_context()
        .dmabuf_render_formats()
        .iter()
        .copied()
        .collect::<smithay::backend::allocator::format::FormatSet>();
    let allocator = GbmAllocator::new(
        opened.gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let import_node = if renders { opened.render_node } else { None };
    let exporter = GbmFramebufferExporter::new(opened.gbm.clone(), import_node);
    let manager = DrmOutputManager::new(
        opened.drm,
        allocator,
        exporter,
        Some(opened.gbm.clone()),
        depth.primary_formats().iter().copied(),
        render_formats,
    );
    let events = handle
        .insert_source(opened.notifier, move |event, metadata, state| match event {
            DrmEvent::VBlank(crtc) => frame_finish(
                state,
                node,
                crtc,
                metadata.as_ref().map(|m| m.time),
                metadata.as_ref().map(|m| m.sequence),
            ),
            DrmEvent::Error(e) => tracing::error!(error = %e, "DRM error"),
        })
        .map_err(|e| anyhow!("inserting the DRM source: {e}"))?;
    Ok(Card {
        devnum,
        prefix,
        renderer: opened.renderer,
        gbm: opened.gbm,
        manager,
        scanner: DrmScanner::new(),
        surfaces: HashMap::new(),
        prepared: HashMap::new(),
        heads: HashMap::new(),
        waiting: HashSet::new(),
        events,
    })
}

/// Open a card that does not render; a scan brings up its displays. A card that cannot be opened is logged and left
/// alone.
fn open_display_card(state: &mut Cupertino, path: &Path, prefix: String) -> Option<DrmNode> {
    let added = (|| -> Result<DrmNode> {
        let node = DrmNode::from_path(path).map_err(|e| anyhow!("drm node: {e}"))?;
        let devnum = DevT::from_raw(node.dev_id());
        let u = udev(state).context("the TTY backend")?;
        let depth = u.depth;
        let opened = open_card(&u.session, path)?;
        let card = card(
            &state.loop_handle,
            node,
            devnum,
            prefix,
            opened,
            false,
            depth,
        )?;
        udev(state)
            .context("the TTY backend")?
            .cards
            .insert(node, card);
        Ok(node)
    })();
    match added {
        Ok(node) => {
            tracing::info!(path = %path.display(), "showing the displays of another GPU");
            Some(node)
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %format!("{e:#}"), "the displays of this GPU are not used");
            None
        }
    }
}

/// A card that does not render went away: its displays go with it.
fn remove_card(state: &mut Cupertino, node: DrmNode) {
    let Some(card) = udev(state).and_then(|u| u.cards.remove(&node)) else {
        return;
    };
    state.loop_handle.remove(card.events);
    for head in card.heads.values() {
        state.disconnect_head(head.head);
    }
    tracing::info!(?node, "a GPU went away with its displays");
}

/// What a DRM uevent does.
#[derive(Debug, PartialEq, Eq)]
enum CardEvent<N> {
    /// A connector of a known card changed: scan it.
    Scan(N),
    /// A card node appeared: open it.
    Open,
    /// A card that does not render went away.
    Remove(N),
    /// The render GPU went away: the session cannot go on drawing.
    RenderGone,
    Ignore,
}

/// Route a DRM uevent of `action`: `card` is the known card its device number matches, `node_is_card` whether its node
/// is a `card*` node (not a render node or a connector).
fn card_event<N: PartialEq>(
    action: &Action,
    card: Option<N>,
    render: N,
    node_is_card: bool,
) -> CardEvent<N> {
    match (action, card) {
        (Action::Changed, Some(node)) => CardEvent::Scan(node),
        (Action::Added, None) if node_is_card => CardEvent::Open,
        (Action::Removed, Some(node)) if node == render => CardEvent::RenderGone,
        (Action::Removed, Some(node)) => CardEvent::Remove(node),
        _ => CardEvent::Ignore,
    }
}

fn drm_uevent(state: &mut Cupertino, event: &DeviceEvent) {
    let Some(u) = udev(state) else { return };
    let card = event.devnum.and_then(|devnum| u.card_of(devnum));
    let node_is_card = event
        .devnode
        .as_deref()
        .and_then(Path::file_name)
        .is_some_and(|name| name.to_string_lossy().starts_with("card"));
    match card_event(&event.action, card, u.render_node, node_is_card) {
        CardEvent::Scan(node) => scan_card(state, node),
        CardEvent::Open => {
            let gpus = match cupertino_devices::gpus(Path::new("/sys")) {
                Ok(gpus) => gpus,
                Err(e) => {
                    tracing::warn!(error = %e, "listing GPUs for a card plugged in failed");
                    return;
                }
            };
            let gpu = gpus
                .iter()
                .find(|g| g.device.devnum() == event.devnum)
                .cloned();
            let primary = cupertino_devices::select_primary(gpus);
            if let (Some(gpu), Some(primary), Some(path)) = (gpu, primary, event.devnode.as_deref())
            {
                if let Some(node) = open_display_card(state, path, card_prefix(&gpu, &primary)) {
                    scan_card(state, node);
                }
            }
        }
        CardEvent::Remove(node) => remove_card(state, node),
        CardEvent::RenderGone => {
            tracing::error!(
                "the GPU that renders went away; nothing can be drawn until the session restarts"
            )
        }
        CardEvent::Ignore => {}
    }
}

/// Start the TTY backend and run until the compositor stops.
/// What the primary GPU's driver needs before the TTY backend takes the display.
pub fn preflight() -> Result<cupertino_devices::preflight::Report> {
    cupertino_devices::preflight::run(Path::new("/sys"))
        .map_err(|e| anyhow!("listing GPUs: {e}"))?
        .context("no GPU found: /sys/class/drm lists no card")
}

pub fn run(
    state: &mut Cupertino,
    event_loop: &mut EventLoop<'static, Cupertino>,
    deadline: Option<Instant>,
    scale: Option<f64>,
) -> Result<()> {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .unwrap_or_default()
            .trim(),
        session = std::env::var("XDG_SESSION_ID").unwrap_or_default(),
        vt = std::env::var("XDG_VTNR").unwrap_or_default(),
        "starting the TTY backend"
    );
    if crate::drm_trace::requested(std::env::var(crate::drm_trace::ENV).ok().as_deref()) {
        state.drm_trace = Some(crate::drm_trace::DrmTrace::new());
        crate::drm_trace::log(
            "on: presentation-time branch, missed flips, planes, frame start delay and input-to-flip and pointer-to-flip latency follow",
        );
    }
    let depth = state.config.output_depth;
    let late_latch = crate::latch::requested(std::env::var(crate::latch::ENV).ok().as_deref());
    tracing::info!(
        primary_plane_bits = depth.bits(),
        late_latch,
        "the primary planes' depth asked for, and whether frames start late after a vblank with a real timestamp"
    );
    let report = preflight()?;
    for note in &report.notes {
        tracing::info!("{note}");
    }
    for warning in &report.warnings {
        tracing::warn!("{warning}");
    }
    if !report.is_ok() {
        for problem in &report.problems {
            tracing::error!("{problem}");
        }
        bail!("this machine is not set up for the TTY backend:\n{report}");
    }
    let (session, notifier) = LogindSession::new().map_err(|e| {
        anyhow!(
            "opening a logind session: {e} (start from a text console such as Ctrl+Alt+F3, or from a display manager's session entry; a running desktop session already controls the seat)"
        )
    })?;
    // Devices are only known on the default seat.
    let seat = session.seat();
    if seat != SEAT0 {
        bail!("the session is on seat {seat}, but devices are only known on {SEAT0}");
    }
    // A session on a virtual terminal in the background holds no DRM master and gets only revoked input devices. It
    // gets ready all the same ([`Preparing`]): taking control of the session put its terminal in graphics mode, so the
    // console draws nothing there when it comes to it, and the GPU, the renderer, the outputs and the first frame are made
    // here, before the session manager is told to switch to it; only the DRM outputs and the input devices wait for the
    // activation.
    let hidden = !session.is_active();
    if hidden {
        tracing::info!(
            "the session is in the background: getting its first frame ready before it is shown"
        );
    }
    let gpus =
        cupertino_devices::gpus(Path::new("/sys")).map_err(|e| anyhow!("listing GPUs: {e}"))?;
    let primary = cupertino_devices::select_primary(gpus.clone()).context("no GPU found")?;
    // a /dev/dri/by-path link names the card that it points to
    let wanted = std::env::var_os("CUPERTINO_DRM_DEVICE")
        .map(PathBuf::from)
        .map(|w| std::fs::canonicalize(&w).unwrap_or(w));
    let (gpu, choice) = cupertino_devices::select_render(gpus.clone(), wanted.as_deref())
        .context("no GPU found")?;
    // a card named by CUPERTINO_DRM_DEVICE renders even in software; another one with a hardware renderer is
    // preferred to the primary one's software renderer
    let rest = match choice {
        cupertino_devices::RenderChoice::Named => Vec::new(),
        cupertino_devices::RenderChoice::Primary => others(&gpus, &gpu),
        cupertino_devices::RenderChoice::Unmatched(wanted) => {
            tracing::warn!(wanted = %wanted.display(), "CUPERTINO_DRM_DEVICE names no GPU; the primary one renders");
            others(&gpus, &gpu)
        }
    };
    let (gpu, opened) = software_render::choose(
        gpu,
        rest,
        |gpu| {
            open_card(
                &session,
                gpu.device.devnode().context("the GPU has no device file")?,
            )
        },
        |opened| opened.software,
    )?;
    let gpu_path = gpu
        .device
        .devnode()
        .context("the GPU has no device file")?
        .to_owned();
    let gpu_devnum = gpu
        .device
        .devnum()
        .context("the GPU has no device number")?;
    tracing::info!(path = %gpu_path.display(), seat = %seat, "rendering on this GPU");
    if opened.software {
        software_render::record(&gpu_path.display().to_string());
    }
    cupertino_diagnostic_events::set_context(
        "renderer",
        cupertino_diagnostic_events::Value::Public(if opened.software {
            "software"
        } else {
            "gles"
        }),
    );

    let node = DrmNode::from_path(&gpu_path)
        .map_err(|e| anyhow!("drm node for {}: {e}", gpu_path.display()))?;
    let mut renderer = opened.renderer;
    state.render = Some(RenderResources::new(&mut renderer)?);
    super::init_buffers(state, &renderer);
    if supports_syncobj_eventfd(&opened.fd) {
        let dh = state.display_handle.clone();
        state.syncobj_state = Some(DrmSyncobjState::new::<Cupertino>(&dh, opened.fd.clone()));
        tracing::info!("explicit sync (linux-drm-syncobj-v1) is offered");
    } else {
        tracing::warn!("explicit sync is not offered: the kernel cannot wait on syncobj timelines");
    }
    let handle = event_loop.handle();
    let card = card(
        &handle,
        node,
        gpu_devnum,
        card_prefix(&gpu, &primary),
        Opened { renderer, ..opened },
        true,
        depth,
    )?;
    state.backend = Some(Backend::Udev(Box::new(UdevBackend {
        session: session.clone(),
        render_node: node,
        cards: HashMap::from([(node, card)]),
        scale,
        paused: hidden,
        preparing: hidden.then(|| Preparing {
            since: Instant::now(),
            told: false,
        }),
        asleep: false,
        renderer_lost: false,
        depth,
        late_latch,
    })));
    state.gpu_recovers = true;
    for other in gpus.iter().filter(|g| g.device != gpu.device) {
        if let Some(path) = other.device.devnode() {
            open_display_card(state, path, card_prefix(other, &primary));
        }
    }

    let inputs = input::new_inputs(handle.clone(), session.clone());

    let session_inputs = inputs.clone();
    handle
        .insert_source(notifier, move |event, &mut (), state| match event {
            SessionEvent::PauseSession => {
                state.revoke_controls(cupertino_control_model::Revocation::SessionDeactivated);
                tracing::info!("session paused");
                state.session_deactivated();
                input::close_all(&session_inputs, state);
                if let Some(u) = udev(state) {
                    u.paused = true;
                    for card in u.cards.values_mut() {
                        card.manager.pause();
                    }
                }
            }
            SessionEvent::ActivateSession => {
                let first = udev(state).is_some_and(|u| u.preparing.is_some());
                if first {
                    tracing::info!("session shown");
                } else {
                    tracing::info!("session resumed");
                }
                if let Err(e) = input::open_all(&session_inputs, state) {
                    tracing::error!(error = %e, "reopening the input devices failed");
                }
                if let Some(u) = udev(state) {
                    u.paused = false;
                    for card in u.cards.values_mut() {
                        if let Err(e) = card.manager.activate(false) {
                            tracing::error!(error = %e, "re-activating the DRM device failed");
                        }
                    }
                    for s in u.cards.values_mut().flat_map(|c| c.surfaces.values_mut()) {
                        // Smithay keeps a frame that waited for its vblank at the pause and holds every later frame of
                        // the CRTC behind it; if that vblank never comes, the head freezes. Another session had the
                        // device, so no flip of ours is under way: the frame counts as shown, and the display keeps
                        // what it shows until the next frame replaces it (clearing the CRTC would blank it)
                        if s.pending {
                            retire_pending_frame(s);
                        }
                        s.pending = false;
                    }
                }
                // the displays enabled while the session was getting ready get their DRM outputs now
                set_up_prepared(state);
                for o in &mut state.outputs {
                    o.scheduler.reset();
                }
                // displays plugged, unplugged or swapped while another session had the seat sent their uevents to it
                scan_connectors(state);
                state.session_activated();
            }
        })
        .map_err(|e| anyhow!("inserting the session source: {e}"))?;

    let monitor = Monitor::new().map_err(|e| anyhow!("opening the uevent socket: {e}"))?;
    let hotplug_inputs = inputs.clone();
    handle
        .insert_source(
            Generic::new(monitor, Interest::READ, Mode::Level),
            move |_, monitor, state| {
                match monitor.drain() {
                    Ok(drained) => {
                        for error in &drained.errors {
                            tracing::warn!(error = %error, "dropping a malformed uevent");
                        }
                        for event in drained.events {
                            if event.subsystem == Subsystem::Input.as_str() {
                                input::hotplug(&hotplug_inputs, &event, state);
                            } else if event.subsystem == Subsystem::Drm.as_str() {
                                drm_uevent(state, &event);
                            }
                        }
                        // Events were lost: the open devices may no longer match the nodes.
                        if drained.overflowed {
                            tracing::warn!(
                                "the uevent queue overflowed, enumerating the input devices and displays again"
                            );
                            input::resync(&hotplug_inputs, state);
                            scan_connectors(state);
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "reading a uevent failed"),
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| anyhow!("inserting the uevent source: {e}"))?;

    // the input devices of a session in the background are revoked: they are opened when it is shown
    if !hidden {
        input::open_all(&inputs, state)?;
    }
    scan_connectors(state);
    if state.outputs.is_empty() {
        return Err(anyhow!("no connected output"));
    }

    let recorder = super::stalls::Recorder::start(event_loop.handle());
    while state.running {
        event_loop
            .dispatch(Some(loop_timeout(state, deadline)), state)
            .map_err(|e| anyhow!("event loop: {e}"))?;
        state.tick();
        render_outputs(state);
        prepare_first_frame(state);
        let _ = state.display_handle.flush_clients();
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
    }
    drop(recorder);
    if let (Some(Backend::Udev(u)), Some(res)) = (state.backend.as_mut(), state.render.as_ref()) {
        res.destroy(u.renderer());
    }
    Ok(())
}

mod outputs;
use outputs::{
    frame_finish, prepare_first_frame, render_outputs, retire_pending_frame, scan_card,
    set_up_prepared, PREPARE_POLL,
};

pub(crate) use outputs::{head_op, scan_connectors};

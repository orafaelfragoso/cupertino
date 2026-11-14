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
use cupertino_render_api::{Completion as _, Failure, RenderFailure as _, RenderStatus, Renderer};
use cupertino_seat::LogindSession;
use cupertino_wm::OutputId;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::drm::compositor::{FrameError, FrameFlags, RenderFrameError};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{
    DrmOutput, DrmOutputManager, DrmOutputManagerError, DrmOutputRenderElements,
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
use smithay::desktop::utils::OutputPresentationFeedback;
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
use crate::latch::LateLatch;
use crate::output_depth::Depth;
use crate::render::gles::{Gles, GlesCompletion, GlesResources, GlesTarget, RenderError};
use crate::render::scene::RenderResources;
use crate::schedule::{Overdue, PresentationId};
use crate::software_render;
use crate::state::Cupertino;

const CLEAR: Color32F = Color32F::new(0.05, 0.05, 0.07, 1.0);

/// The DRM output of one display, as the renderer draws to it.
pub(crate) type DrmSurfaceOutput =
    DrmOutput<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, Frame, DrmDeviceFd>;
type Manager = DrmOutputManager<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    Frame,
    DrmDeviceFd,
>;

/// What a queued flip carries to its vblank.
pub struct Frame {
    /// The scheduler's id for it.
    id: PresentationId,
    feedback: OutputPresentationFeedback,
    /// The client buffers it was drawn from. Dropping the last handle of one signals its explicit-sync release point,
    /// which must wait until the GPU finished reading it: once the flip is done, the frame drawn from it is finished.
    _buffers: Vec<Buffer>,
    /// Its number in the DRM trace, when the trace is on ([`crate::drm_trace::DrmTrace::queued`]).
    trace: Option<u64>,
}

struct Surface {
    output: OutputId,
    drm_output: DrmSurfaceOutput,
    /// On a card that does not render: the copies of the render GPU's frames it shows.
    copy: Option<FrameCopy<DrmDeviceFd>>,
    /// A frame was queued and its vblank has not arrived yet.
    pending: bool,
    refresh: Duration,
    /// The recent render times, and how late after a vblank the next frame may start ([`crate::latch`]).
    latch: LateLatch,
    /// The vblank the frame in flight was drawn for, where the vblank phase was known when it started.
    target: Option<Duration>,
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
/// draw (paused, asleep, off) wait for an event, not a frame deadline.
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
    super::loop_wait([tick, frames, end])
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
    let (drm, notifier) =
        DrmDevice::new(fd.clone(), true).map_err(|e| anyhow!("drm device: {e}"))?;
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
    let (session, mut notifier) = LogindSession::new().map_err(|e| {
        anyhow!(
            "opening a logind session: {e} (start from a text console such as Ctrl+Alt+F3, or from a display manager's session entry; a running desktop session already controls the seat)"
        )
    })?;
    // Devices are only known on the default seat.
    let seat = session.seat();
    if seat != SEAT0 {
        bail!("the session is on seat {seat}, but devices are only known on {SEAT0}");
    }
    // A session on a virtual terminal in the background holds no DRM master and gets only
    // revoked input devices. Only the session's own events run until it activates, on a loop of
    // their own: the compositor's sources (shell processes, clients) need the outputs first.
    if !session.is_active() {
        tracing::info!("waiting for the session to become active");
        let mut waiting: EventLoop<'_, ()> =
            EventLoop::try_new().context("creating the session wait loop")?;
        waiting
            .handle()
            .insert_source(&mut notifier, |_, &mut (), _| {})
            .map_err(|e| anyhow!("inserting the session source: {e}"))?;
        while !session.is_active() {
            waiting
                .dispatch(None, &mut ())
                .map_err(|e| anyhow!("waiting for the session: {e}"))?;
        }
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
        paused: false,
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
                input::close_all(&session_inputs, state);
                if let Some(u) = udev(state) {
                    u.paused = true;
                    for card in u.cards.values_mut() {
                        card.manager.pause();
                    }
                }
            }
            SessionEvent::ActivateSession => {
                tracing::info!("session resumed");
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
                        // device, so no flip of ours is under way: clearing the CRTC drops the frame, and the next one
                        // is a full modeset (as after any resume)
                        if s.pending {
                            drop_pending_frame(s, "the pause");
                        }
                        s.pending = false;
                    }
                }
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

    input::open_all(&inputs, state)?;
    scan_connectors(state);
    if state.outputs.is_empty() {
        return Err(anyhow!("no connected output"));
    }

    while state.running {
        event_loop
            .dispatch(Some(loop_timeout(state, deadline)), state)
            .map_err(|e| anyhow!("event loop: {e}"))?;
        state.tick();
        render_outputs(state);
        let _ = state.display_handle.flush_clients();
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
    }
    if let (Some(Backend::Udev(u)), Some(res)) = (state.backend.as_mut(), state.render.as_ref()) {
        res.destroy(u.renderer());
    }
    Ok(())
}

/// Scan the connectors of every card ([`scan_card`]).
pub(crate) fn scan_connectors(state: &mut Cupertino) {
    let nodes: Vec<DrmNode> =
        udev(state).map_or_else(Vec::new, |u| u.cards.keys().copied().collect());
    for node in nodes {
        scan_card(state, node);
    }
}

/// Bring the heads of a card in line with its connectors: every connected connector with a CRTC is driven, and nothing
/// else.
/// Smithay's scanner reports a connector once, when it connects; a connector that waited for a CRTC gets one later
/// without an event (another display let it go), and a display swapped on the same connector while the session was away
/// is no event either. So the heads are compared with the scanner's whole mapping, and a driven connector whose display
/// is another one (its EDID changed) is connected afresh.
fn scan_card(state: &mut Cupertino, node: DrmNode) {
    let Some(Backend::Udev(u)) = state.backend.as_mut() else {
        return;
    };
    // a paused session cannot set a mode; its activation scans again
    if u.paused {
        return;
    }
    let Some(u) = u.cards.get_mut(&node) else {
        return;
    };
    if let Err(e) = u.scanner.scan_connectors(u.manager.device()) {
        tracing::warn!(error = %e, "scanning connectors failed");
        return;
    }
    let mapped: Vec<(crtc::Handle, connector::Info)> = u
        .scanner
        .crtcs()
        .filter(|(info, _)| info.state() == connector::State::Connected)
        .map(|(info, crtc)| (crtc, info.clone()))
        .collect();
    let mut waiting = HashSet::new();
    for (handle, info) in u.scanner.connectors() {
        if info.state() == connector::State::Connected
            && u.scanner.crtc_for_connector(handle).is_none()
        {
            if !u.waiting.contains(handle) {
                tracing::warn!(connector = %format!("{}{}", u.prefix, connector_name(info)), "a display is connected but every CRTC is in use: it stays off until another display is unplugged");
            }
            waiting.insert(*handle);
        }
    }
    u.waiting = waiting;
    let device = u.manager.device();
    let driven: Vec<(crtc::Handle, connector::Handle)> = u
        .heads
        .iter()
        .filter(|(_, h)| {
            // an EDID that cannot be read now is no other display
            connector_edid(device, h.connector).is_none_or(|now| Some(now) == h.edid)
        })
        .map(|(crtc, h)| (*crtc, h.connector))
        .collect();
    let wanted: Vec<(crtc::Handle, connector::Handle)> = mapped
        .iter()
        .map(|(crtc, info)| (*crtc, info.handle()))
        .collect();
    let mut gone: Vec<crtc::Handle> = u
        .heads
        .keys()
        .filter(|crtc| !driven.iter().any(|(c, _)| c == *crtc))
        .copied()
        .collect();
    let (dropped, added) = reconcile(&driven, &wanted);
    gone.extend(dropped);
    for crtc in gone {
        let Some(u) = udev(state).and_then(|u| u.cards.get_mut(&node)) else {
            return;
        };
        u.surfaces.remove(&crtc);
        if let Some(head) = u.heads.remove(&crtc) {
            state.disconnect_head(head.head);
        }
    }
    for (crtc, connector) in added {
        if let Some((_, info)) = mapped
            .iter()
            .find(|(c, info)| *c == crtc && info.handle() == connector)
        {
            connector_connected(state, node, info.clone(), crtc);
        }
    }
}

/// What a scan changes, from the (CRTC, connector) pairs driven and the pairs mapped now: the CRTCs whose head goes, then
/// the pairs to bring up.
fn reconcile<C: Copy + PartialEq, K: Copy + PartialEq>(
    driven: &[(C, K)],
    mapped: &[(C, K)],
) -> (Vec<C>, Vec<(C, K)>) {
    let gone = driven
        .iter()
        .filter(|pair| !mapped.contains(pair))
        .map(|(crtc, _)| *crtc)
        .collect();
    let new = mapped
        .iter()
        .filter(|pair| !driven.contains(pair))
        .copied()
        .collect();
    (gone, new)
}

fn connector_name(connector: &connector::Info) -> String {
    format!(
        "{}-{}",
        connector.interface().as_str(),
        connector.interface_id()
    )
}

/// Refresh rate of a DRM mode in millihertz, from its pixel clock and totals (an interlaced mode
/// shows two fields per frame, a double-scanned one half; `vscan` repeats lines).
fn refresh_mhz(mode: &control::Mode) -> i32 {
    let (_, _, htotal) = mode.hsync();
    let (_, _, vtotal) = mode.vsync();
    if htotal == 0 || vtotal == 0 {
        return mode.vrefresh() as i32 * 1000;
    }
    let mut refresh = (u64::from(mode.clock()) * 1_000_000 / u64::from(htotal)
        + u64::from(vtotal) / 2)
        / u64::from(vtotal);
    let flags = mode.flags();
    if flags.contains(ModeFlags::INTERLACE) {
        refresh *= 2;
    }
    if flags.contains(ModeFlags::DBLSCAN) {
        refresh /= 2;
    }
    if mode.vscan() > 1 {
        refresh /= u64::from(mode.vscan());
    }
    refresh as i32
}

/// The EDID blob of a connector, when it has one.
fn connector_edid(device: &impl ControlDevice, connector: connector::Handle) -> Option<Vec<u8>> {
    let props = device.get_properties(connector).ok()?;
    let (handles, values) = props.as_props_and_values();
    handles.iter().zip(values).find_map(|(handle, value)| {
        let info = device.get_property(*handle).ok()?;
        if info.name().to_bytes() != b"EDID" {
            return None;
        }
        let blob = info
            .value_type()
            .convert_value(*value)
            .as_blob()
            .filter(|b| *b != 0)?;
        device.get_property_blob(blob).ok()
    })
}

fn connector_connected(
    state: &mut Cupertino,
    node: DrmNode,
    connector: connector::Info,
    crtc: crtc::Handle,
) {
    let Some(Backend::Udev(u)) = state.backend.as_mut() else {
        return;
    };
    let scale = u.scale;
    let Some(u) = u.cards.get_mut(&node) else {
        return;
    };
    let name = format!("{}{}", u.prefix, connector_name(&connector));
    let preferred = connector
        .modes()
        .iter()
        .position(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
        .unwrap_or(0);
    let mut modes: Vec<(HeadMode, control::Mode)> = Vec::new();
    for (i, m) in connector.modes().iter().enumerate() {
        let (w, h) = m.size();
        let mode = HeadMode {
            width: i32::from(w),
            height: i32::from(h),
            refresh_mhz: refresh_mhz(m),
        };
        if modes.iter().any(|(known, _)| *known == mode) {
            continue;
        }
        if i == preferred {
            modes.insert(0, (mode, *m));
        } else {
            modes.push((mode, *m));
        }
    }
    if modes.is_empty() {
        tracing::warn!(connector = %name, "no modes");
        return;
    }
    let edid = connector_edid(u.manager.device(), connector.handle());
    let identity = edid.as_deref().and_then(edid::parse);
    let (make, model, serial) = match identity {
        Some(id) => (id.make, id.model, id.serial),
        None => ("Unknown".to_string(), name.clone(), None),
    };
    let physical_mm = connector
        .size()
        .map_or((0, 0), |(w, h)| (w as i32, h as i32));
    let info = HeadInfo {
        connector: name,
        make,
        model,
        serial,
        physical_mm,
        modes: modes.iter().map(|(m, _)| *m).collect(),
        caps: HeadCaps {
            can_disable: true,
            custom_modes: false,
            transforms: true,
        },
        base_transform: Transform::Normal,
    };
    let (head, initial) = state.connect_head(info, scale);
    let Some(u) = udev(state).and_then(|u| u.cards.get_mut(&node)) else {
        return;
    };
    u.heads.insert(
        crtc,
        DrmHead {
            head,
            connector: connector.handle(),
            edid,
            modes,
        },
    );
    if initial.enabled {
        if let Err(e) = state.enable_head(head) {
            tracing::warn!(error = %e, "enabling the display failed");
        }
        state.heads_changed();
    }
}

/// Carry out `op` on a DRM head.
pub(crate) fn head_op(
    state: &mut Cupertino,
    u: &mut UdevBackend,
    head: HeadId,
    op: HeadOp,
) -> Result<()> {
    let (node, crtc) = u.head_place(head).context("the head has no connector")?;
    let u = u.cards.get_mut(&node).expect("the head's card");
    let drm_head = &u.heads[&crtc];
    match op {
        HeadOp::Enable => {
            let mode = state
                .displays
                .head(head)
                .context("the head being enabled")?
                .state
                .mode;
            let drm_mode = drm_head.drm_mode(mode)?;
            let connector = drm_head.connector;
            let output = state.prepare_head_output(head)?;
            let elements =
                DrmOutputRenderElements::<GlesRenderer, crate::render::CupertinoElement>::default();
            let mut planes = u.manager.device().planes(&crtc).ok();
            if let Some(p) = planes.as_mut() {
                // overlay planes misbehave on the proprietary NVIDIA driver
                if u.manager
                    .device()
                    .get_driver()
                    .map(|d| d.name().to_string_lossy().to_lowercase().contains("nvidia"))
                    .unwrap_or(false)
                {
                    p.overlay = vec![];
                }
            }
            match u.manager.initialize_output(
                crtc,
                drm_mode,
                &[connector],
                &output,
                planes,
                &mut u.renderer,
                &elements,
            ) {
                Ok(drm_output) => {
                    let id = state.create_head_output(head, output.clone());
                    let refresh =
                        Duration::from_secs_f64(1000.0 / mode.refresh_mhz.max(1000) as f64);
                    u.surfaces.insert(
                        crtc,
                        Surface {
                            output: id,
                            drm_output,
                            copy: None,
                            pending: false,
                            refresh,
                            latch: LateLatch::default(),
                            target: None,
                        },
                    );
                    let format = u.surfaces[&crtc].drm_output.format();
                    tracing::info!(output = %output.name(), width = mode.width, height = mode.height, refresh_mhz = mode.refresh_mhz, format = %format, bits = crate::output_depth::format_bits(format), "output initialised");
                    Ok(())
                }
                Err(e) => {
                    // a paused session is busy for every mode, as a device another program holds
                    let paused = matches!(
                        e,
                        DrmOutputManagerError::Drm(DrmError::DeviceInactive)
                            | DrmOutputManagerError::Frame(FrameError::DrmError(
                                DrmError::DeviceInactive
                            ))
                    );
                    let e = if paused {
                        anyhow::Error::new(std::io::Error::new(ErrorKind::ResourceBusy, e))
                    } else {
                        anyhow::Error::new(e)
                    };
                    Err(e.context(format!("initialising {}", output.name())))
                }
            }
        }
        HeadOp::Disable => {
            // dropping the DRM output turns its CRTC off
            u.surfaces.remove(&crtc);
            state.drop_head_output(head);
            Ok(())
        }
        HeadOp::SetMode(mode) => {
            let drm_mode = drm_head.drm_mode(mode)?;
            let surface = u
                .surfaces
                .get_mut(&crtc)
                .context("the head is not enabled")?;
            // a frame waiting for its vblank would stay Smithay's pending frame across the mode change and hold every later
            // frame behind it, as after a pause; clearing the CRTC drops it, and the new mode starts with a full modeset
            if surface.pending {
                drop_pending_frame(surface, "the mode change");
            }
            let elements =
                DrmOutputRenderElements::<GlesRenderer, crate::render::CupertinoElement>::default();
            surface
                .drm_output
                .use_mode(drm_mode, &mut u.renderer, &elements)
                .map_err(|e| anyhow!("switching the mode: {e}"))?;
            surface.refresh = Duration::from_secs_f64(1000.0 / mode.refresh_mhz.max(1000) as f64);
            surface.pending = false;
            reset_frames(state, surface.output);
            Ok(())
        }
        HeadOp::Power(on) => {
            let surface = u
                .surfaces
                .get_mut(&crtc)
                .context("the head is not enabled")?;
            if !on {
                // blanks the CRTC; the next queued frame turns it on again
                surface
                    .drm_output
                    .with_compositor(|c| c.clear())
                    .map_err(|e| anyhow!("blanking the output: {e}"))?;
                surface.pending = false;
                reset_frames(state, surface.output);
            }
            Ok(())
        }
    }
}

/// Drop the frame waiting for its vblank on `surface` by clearing its CRTC, before `what`. The commit is blocking: a
/// driver that lost the flip may hold it until its own timeout (up to 3 s on nvidia-drm), so the time it took is
/// logged. Returns whether the frame is gone.
fn drop_pending_frame(surface: &mut Surface, what: &str) -> bool {
    let started = Instant::now();
    let cleared = surface.drm_output.with_compositor(|c| c.clear());
    let took = started.elapsed();
    match cleared {
        Ok(()) => {
            tracing::info!(?took, "dropped the frame queued before {what}");
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, ?took, "dropping the frame queued before {what} failed");
            false
        }
    }
}

/// The flip in flight on `output` ended without its vblank: a late event for it is stale, and the next frame is full.
fn reset_frames(state: &mut Cupertino, output: OutputId) {
    if let Some(o) = state.outputs.iter_mut().find(|o| o.id == output) {
        o.scheduler.reset();
    }
}

fn render_outputs(state: &mut Cupertino) {
    let Some(mut backend) = state.backend.take() else {
        return;
    };
    let mut unsupported = Vec::new();
    if let Backend::Udev(u) = &mut backend {
        if !(u.paused || u.asleep) {
            let loss = crate::gpu_reset::status(u.renderer())
                .or_else(|| state.gpu_reset.take().map(Loss::Reset))
                .or_else(|| {
                    u.renderer_lost
                        .then(|| Loss::Reset("the rendering context was lost".to_owned()))
                });
            if let Some(loss) = loss {
                if !recover_gpu(state, u, &loss) {
                    state.backend = Some(backend);
                    return;
                }
            }
        }
        let places: Vec<(DrmNode, crtc::Handle)> = u
            .cards
            .iter()
            .flat_map(|(node, card)| card.surfaces.keys().map(|crtc| (*node, *crtc)))
            .collect();
        let now = state.frame_now();
        for (node, crtc) in places {
            let Some(s) = u.cards.get(&node).and_then(|c| c.surfaces.get(&crtc)) else {
                continue;
            };
            let Some(idx) = state.outputs.iter().position(|o| o.id == s.output) else {
                continue;
            };
            if s.pending && !(u.paused || u.asleep) {
                let overdue = state.outputs[idx].scheduler.overdue(now, s.refresh);
                if let Some(overdue) = overdue {
                    flip_overdue(state, u, node, crtc, idx, overdue);
                }
                continue;
            }
            if !state.displays.powered(s.output)
                || !frame_due(
                    u.paused || u.asleep || u.renderer_lost,
                    s.pending,
                    state.frame_due(idx, now),
                )
            {
                continue;
            }
            unsupported.extend(render_surface(state, u, node, crtc, idx));
        }
    }
    state.backend = Some(backend);
    for head in unsupported {
        state.head_unsupported(head);
    }
}

/// The rendering context was lost (`loss`): every card gets a new context and renderer on its EGL display, its displays'
/// buffers and copies are made again, and so is every GPU object of the scene; client buffers are imported again by the
/// next frame. Whether the session goes on: too many resets in a row, or a renderer that cannot be made, end it.
fn recover_gpu(state: &mut Cupertino, u: &mut UdevBackend, loss: &Loss) -> bool {
    let reason = loss.reason();
    let gpu = u
        .render_node
        .dev_path()
        .map_or_else(|| u.render_node.to_string(), |p| p.display().to_string());
    let recovery = if state.gpu_resets.allow(loss, Instant::now()) {
        new_renderers(state, u).map_err(|e| format!("{e:#}"))
    } else {
        Err("the GPU was reset too often within a minute".to_owned())
    };
    crate::gpu_reset::record(&gpu, reason, &recovery);
    match recovery {
        Ok(()) => {
            u.renderer_lost = false;
            state.gpu_reset = None;
            true
        }
        Err(e) => {
            u.renderer_lost = true;
            state.end_for_lost_context(&format!("{reason}; {e}"));
            false
        }
    }
}

fn new_renderers(state: &mut Cupertino, u: &mut UdevBackend) -> Result<()> {
    for card in u.cards.values_mut() {
        let display = card.renderer.egl_context().display().clone();
        let context =
            EGLContext::new_with_reset_notification(&display, Some(ContextPriority::High))
                .map_err(|e| anyhow!("EGL context: {e}"))?;
        card.renderer =
            unsafe { super::gles_renderer(context) }.map_err(|e| anyhow!("GLES renderer: {e}"))?;
        for s in card.surfaces.values_mut() {
            s.copy = None;
            s.drm_output.reset_buffers();
        }
    }
    state.rebuild_render(u.renderer())
}

/// A flip whose vblank is overdue: its clients are called back once per refresh, so none waits on a dead head, and after
/// a second without the vblank the flip is dropped and the head starts over with a full frame. A head whose flip cannot
/// be dropped stays stuck with paced callbacks until a sleep, a resume, a mode change or an unplug.
fn flip_overdue(
    state: &mut Cupertino,
    u: &mut UdevBackend,
    node: DrmNode,
    crtc: crtc::Handle,
    idx: usize,
    overdue: Overdue,
) {
    let Some(surface) = u
        .cards
        .get_mut(&node)
        .and_then(|c| c.surfaces.get_mut(&crtc))
    else {
        return;
    };
    let name = state.outputs[idx].output.name();
    match overdue {
        Overdue::TimedOut => {
            tracing::warn!(output = %name, "the display has not shown the queued frame; calling its clients back once per refresh")
        }
        Overdue::CallBack => {}
        Overdue::GiveUp => {
            tracing::warn!(output = %name, "the display has not shown the queued frame for a second; dropping it");
            if drop_pending_frame(surface, "the head was given up") {
                surface.pending = false;
                state.outputs[idx].scheduler.reset();
                return;
            }
            tracing::error!(output = %name, "the head is stuck until it sleeps, resumes, changes mode or is unplugged");
        }
    }
    let time = state.frame_now();
    state.send_frames(idx, time);
}

/// Whether the frame already on the screen is what this frame would show, so it may be reported as shown: only when a
/// scene was composed and drawn and nothing differed from it. Without a scene nothing says what the screen shows, and
/// the session lock must not be confirmed by it.
fn already_shown(scene: bool, rendered: bool) -> bool {
    scene && !rendered
}

/// What drawing a frame of a display came to.
struct Drawn {
    /// A scene was composed and drawn, so the screen's content is known ([`already_shown`]).
    scene: bool,
    /// Something differs from the frame before: it waits to be queued.
    rendered: bool,
    failed: Option<RenderError>,
    /// The client buffers of the surfaces the frame was drawn from ([`crate::frame::drawn_buffers`]).
    buffers: Vec<Buffer>,
}

impl Drawn {
    const NOTHING: Drawn = Drawn {
        scene: false,
        rendered: false,
        failed: None,
        buffers: Vec::new(),
    };

    /// The outcome of a render of the scene of output `idx` from `buffers`.
    fn of(
        state: &mut Cupertino,
        idx: usize,
        result: Result<bool, RenderError>,
        buffers: Vec<Buffer>,
        lost: &mut bool,
    ) -> Self {
        state.outputs[idx].scheduler.rendered();
        match result {
            Ok(rendered) => {
                state.frame_drawn(idx);
                if !rendered {
                    state.outputs[idx].scheduler.empty();
                }
                Drawn {
                    scene: true,
                    rendered,
                    failed: None,
                    buffers,
                }
            }
            Err(e) => {
                *lost |= e.failure() == Failure::ContextLost;
                Drawn {
                    failed: Some(e),
                    ..Drawn::NOTHING
                }
            }
        }
    }
}

/// Draw and queue a frame of output `idx` on the display at `crtc` of card `node`. Returns the head when its display
/// turned out unsupported: it cannot show the render GPU's frames.
fn render_surface(
    state: &mut Cupertino,
    u: &mut UdevBackend,
    node: DrmNode,
    crtc: crtc::Handle,
    idx: usize,
) -> Option<HeadId> {
    // the frame's render time runs from here to its flip being queued, and the vblank it is drawn for is the first after
    // this moment
    let started = Instant::now();
    let target = {
        let refresh = state.refresh_of(idx);
        let now = state.frame_now();
        state.outputs[idx].scheduler.presentation_at(now, refresh)
    };
    // without a scene (no render resources or no mode yet) or a drawn frame, nothing is queued, but the tail still runs
    let drawn = if node == u.render_node {
        draw_direct(state, u, node, crtc, idx)?
    } else {
        match draw_copy(state, u, node, crtc, idx)? {
            Ok(drawn) => drawn,
            Err(Unsupported) => {
                return u.cards[&node].heads.get(&crtc).map(|h| h.head);
            }
        }
    };
    let surface = u.cards.get_mut(&node)?.surfaces.get_mut(&crtc)?;
    let time = Duration::from(state.clock.now());
    if drawn.rendered {
        let feedback = state.collect_feedback(idx);
        let id = state.outputs[idx].scheduler.next_presentation();
        let name = state
            .drm_trace
            .is_some()
            .then(|| state.outputs[idx].output.name());
        let trace = state
            .drm_trace
            .as_mut()
            .zip(name)
            .map(|(t, name)| t.queued(&name));
        let frame = Frame {
            id,
            feedback,
            _buffers: drawn.buffers,
            trace,
        };
        match surface.drm_output.queue_frame(frame) {
            Ok(()) => {
                surface.pending = true;
                surface.latch.rendered(started.elapsed());
                surface.target = target;
                let locked = state.scene_built_locked(idx);
                let now = state.frame_now();
                state.outputs[idx].scheduler.flip_queued(now, locked);
            }
            // logind drops DRM master before it sends the pause, so a flip in between is refused; the pause follows
            Err(FrameError::DrmError(DrmError::Access(e)))
                if e.source.kind() == ErrorKind::PermissionDenied =>
            {
                tracing::debug!(error = %e, "the page flip was refused: the session is being paused")
            }
            Err(e) => {
                tracing::warn!(output = %state.outputs[idx].output.name(), error = %e, "queueing the frame failed")
            }
        }
    }
    let refresh = surface.refresh;
    if already_shown(drawn.scene, drawn.rendered) {
        state.note_frame_shown(idx);
    }
    state.process_screenshots(u.renderer(), idx);
    match drawn.failed {
        Some(e) => state.frame_failed(idx, time, refresh, &e),
        None => state.send_frames(idx, time),
    }
    None
}

/// Draw a frame of a display of the render GPU: its DRM compositor draws the scene's elements.
fn draw_direct(
    state: &mut Cupertino,
    u: &mut UdevBackend,
    node: DrmNode,
    crtc: crtc::Handle,
    idx: usize,
) -> Option<Drawn> {
    let card = u.cards.get_mut(&node)?;
    let Some(composed) = state.compose_for_present(&mut card.renderer, idx) else {
        return Some(Drawn::NOTHING);
    };
    let surface = card.surfaces.get_mut(&crtc)?;
    let buffers = crate::frame::drawn_buffers(&composed.resources.surfaces);
    let (res, cache, _) = state.gles_parts(idx)?;
    let mut resources = GlesResources::new(res, cache, composed.resources);
    let result = Gles.render(
        &composed.frame,
        &mut resources,
        GlesTarget::Drm {
            renderer: &mut card.renderer,
            output: &mut surface.drm_output,
            clear: CLEAR,
        },
    );
    drop(resources);
    if let (Ok(GlesCompletion::Direct { planes, .. }), Some(trace)) =
        (&result, state.drm_trace.as_mut())
    {
        let name = state.outputs[idx].output.name();
        if let Some(line) = trace.planes(&name, *planes) {
            crate::drm_trace::log(&line);
        }
    }
    let result = result.map(|completion| completion.status() == RenderStatus::Rendered);
    Some(Drawn::of(state, idx, result, buffers, &mut u.renderer_lost))
}

/// Draw a frame of a display of another card: the render GPU draws the scene into a buffer of the display's copy, and
/// the display's card draws that buffer ([`copy`]).
fn draw_copy(
    state: &mut Cupertino,
    u: &mut UdevBackend,
    node: DrmNode,
    crtc: crtc::Handle,
    idx: usize,
) -> Option<Result<Drawn, Unsupported>> {
    let [Some(render), Some(card)] = u.cards.get_disjoint_mut([&u.render_node, &node]) else {
        return None;
    };
    let output = &state.outputs[idx].output;
    let mode = output.current_mode()?;
    let size = output.current_transform().transform_size(mode.size);
    let scale = output.current_scale().fractional_scale();
    let surface = card.surfaces.get_mut(&crtc)?;
    let copy = match &mut surface.copy {
        Some(copy) if copy.fits(size, scale) => copy,
        slot => {
            let via = match slot {
                Some(copy) => copy.via(),
                None => copy::first_via(&render.renderer, &card.renderer),
            };
            let allocator = GbmAllocator::new(render.gbm.clone(), GbmBufferFlags::RENDERING);
            slot.insert(FrameCopy::new(allocator, size, scale, via))
        }
    };
    let mut target = match copy.acquire(&mut render.renderer) {
        Ok(acquired) => acquired,
        Err(_) if copy.via() == Via::Unsupported => return Some(Err(Unsupported)),
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "no buffer to draw the display's frame into");
            return Some(Ok(Drawn::NOTHING));
        }
    };
    let Some(composed) = state.compose_for_present(&mut render.renderer, idx) else {
        return Some(Ok(Drawn::NOTHING));
    };
    let buffers = crate::frame::drawn_buffers(&composed.resources.surfaces);
    let (res, cache, _) = state.gles_parts(idx)?;
    let mut resources = GlesResources::new(res, cache, composed.resources);
    let result = match &mut target {
        copy::Target::Dmabuf { slot, dmabuf } => Gles.render(
            &composed.frame,
            &mut resources,
            GlesTarget::Dmabuf {
                renderer: &mut render.renderer,
                dmabuf,
                tracker: &mut copy.tracker,
                age: usize::from(slot.age()),
                clear: CLEAR,
            },
        ),
        copy::Target::Texture { texture, age } => Gles.render(
            &composed.frame,
            &mut resources,
            GlesTarget::Texture {
                renderer: &mut render.renderer,
                texture,
                tracker: &mut copy.tracker,
                age: *age,
                clear: CLEAR,
            },
        ),
    };
    let drawn = match result {
        Ok(GlesCompletion::Offscreen {
            damage: Some(damage),
            sync,
        }) => Ok(Some((damage, sync))),
        Ok(GlesCompletion::Drawn {
            damage: Some(damage),
        }) => Ok(Some((damage, SyncPoint::signaled()))),
        Ok(_) => Ok(None),
        Err(e) => Err(e),
    };
    let result = match drawn {
        Ok(Some((damage, sync))) => {
            let copied = match copy.bring(
                &mut render.renderer,
                &mut card.renderer,
                target,
                sync,
                &damage,
            ) {
                Ok(copied) => copied,
                Err(Unsupported) => return Some(Err(Unsupported)),
            };
            surface
                .drm_output
                .render_frame(&mut card.renderer, &[copied], CLEAR, FrameFlags::DEFAULT)
                .map(|r| !r.is_empty)
                .map_err(|e| match e {
                    RenderFrameError::PrepareFrame(e) => RenderError::Rendering(e.into()),
                    RenderFrameError::RenderFrame(e) => RenderError::Target(format!("{e:?}")),
                })
        }
        Ok(_) => Ok(false),
        // the render GPU cannot draw into the copy's buffer: the next frame tries another kind, or the display goes
        Err(RenderError::Target(e)) => {
            copy.cannot_draw(&e);
            if copy.via() == Via::Unsupported {
                return Some(Err(Unsupported));
            }
            Err(RenderError::Target(e))
        }
        Err(e) => Err(e),
    };
    Some(Ok(Drawn::of(
        state,
        idx,
        result,
        buffers,
        &mut u.renderer_lost,
    )))
}

fn frame_finish(
    state: &mut Cupertino,
    node: DrmNode,
    crtc: crtc::Handle,
    time: Option<DrmEventTime>,
    seq: Option<u32>,
) {
    let hardware = match time {
        Some(DrmEventTime::Monotonic(t)) if !t.is_zero() => Some(t),
        _ => None,
    };
    let flags = if hardware.is_some() {
        wp_presentation_feedback::Kind::Vsync
            | wp_presentation_feedback::Kind::HwClock
            | wp_presentation_feedback::Kind::HwCompletion
    } else {
        wp_presentation_feedback::Kind::Vsync
    };
    let clock = hardware.map_or_else(|| state.clock.now(), Into::into);
    let Some(u) = udev(state) else { return };
    let Some(surface) = u
        .cards
        .get_mut(&node)
        .and_then(|c| c.surfaces.get_mut(&crtc))
    else {
        return;
    };
    surface.pending = false;
    let (output, refresh) = (surface.output, surface.refresh);
    let target = surface.target.take();
    // `Ok(None)`: no frame of ours waited (it was dropped with its CRTC's state). An error comes after Smithay took the
    // shown frame, whose id goes with it, and failed to submit the frame parked behind it (queued while a dropped frame
    // was still pending): that is the flip in flight, and it never reaches the screen
    let (shown, failed) = match surface.drm_output.frame_submitted() {
        Ok(shown) => (shown, false),
        Err(e) => {
            tracing::warn!(error = %e, "frame submission failed");
            (None, true)
        }
    };
    let Some(idx) = state.outputs.iter().position(|o| o.id == output) else {
        return;
    };
    let scheduler = &mut state.outputs[idx].scheduler;
    if failed {
        scheduler.reset();
    }
    // only the flip in flight is shown: a late event of a flip dropped by a reset confirms no frame, a lock frame least
    // of all, and the refresh grid does not follow it; its feedback is discarded
    let presented = shown.as_ref().and_then(|f| scheduler.presented(f.id));
    // the late start needs the driver's timestamps on the presentation clock, which a test's fixed time is not
    let stamp = hardware.filter(|_| state.fixed_time.is_none());
    let late_latch = udev(state).is_some_and(|u| u.late_latch);
    let flipped = match (presented.is_some(), udev(state)) {
        (true, Some(u)) => u
            .cards
            .get_mut(&node)
            .and_then(|c| c.surfaces.get_mut(&crtc))
            .map(|s| s.latch.flipped(stamp, target, refresh, late_latch)),
        _ => None,
    };
    if let (Some(presented), Some(trace)) = (presented.as_ref(), state.drm_trace.as_mut()) {
        let now = std::time::Instant::now();
        let flip = hardware.map_or(now, |t| {
            crate::drm_trace::instant_of(t, now, Duration::from(state.clock.now()))
        });
        let branch = if hardware.is_some() {
            crate::drm_trace::Branch::FlipTimestamp
        } else {
            crate::drm_trace::Branch::OwnClock
        };
        let frame = shown.as_ref().and_then(|f| f.trace).unwrap_or(0);
        let name = state.outputs[idx].output.name();
        let late = presented.late_periods.max(flipped.map_or(0, |f| f.late));
        for line in trace.flipped(&name, branch, late, frame, flip) {
            crate::drm_trace::log(&line);
        }
        let delay = flipped
            .and_then(|f| f.start_at)
            .zip(stamp)
            .map_or(Duration::ZERO, |(at, vblank)| at - vblank);
        if let Some(line) = trace.start_delay(&name, delay) {
            crate::drm_trace::log(&line);
        }
    }
    if let Some(presented) = presented {
        if let Some(Frame { mut feedback, .. }) = shown {
            feedback.presented(
                clock,
                Refresh::fixed(refresh),
                seq.unwrap_or(0) as u64,
                flags,
            );
        }
        if presented.late_periods > 0 {
            tracing::info!(
                late_periods = presented.late_periods,
                "the display showed the queued frame late"
            );
        }
        let vblank = match hardware {
            Some(t) if state.fixed_time.is_none() => t,
            _ => state.frame_now(),
        };
        state.outputs[idx].scheduler.vblank(vblank, refresh);
        state.outputs[idx]
            .scheduler
            .hold_until(flipped.and_then(|f| f.start_at));
        state.note_frame_presented(idx, presented.locked);
    }
    state.outputs[idx].scheduler.damage();
}

#[cfg(test)]
mod tests {
    use super::{already_shown, card_event, frame_due, reconcile, CardEvent};
    use cupertino_devices::Action;

    #[test]
    fn a_drm_uevent_goes_to_the_card_its_device_number_names() {
        // card 1 renders, card 2 shows displays
        assert_eq!(
            card_event(&Action::Changed, Some(2), 1, true),
            CardEvent::Scan(2)
        );
        assert_eq!(
            card_event(&Action::Changed, None, 1, true),
            CardEvent::Ignore
        );
        assert_eq!(card_event(&Action::Added, None, 1, true), CardEvent::Open);
        assert_eq!(
            card_event(&Action::Added, None, 1, false),
            CardEvent::Ignore,
            "a render node or a connector is no card"
        );
        assert_eq!(
            card_event(&Action::Added, Some(2), 1, true),
            CardEvent::Ignore,
            "a card already open"
        );
        assert_eq!(
            card_event(&Action::Removed, Some(2), 1, true),
            CardEvent::Remove(2)
        );
        assert_eq!(
            card_event(&Action::Removed, Some(1), 1, true),
            CardEvent::RenderGone
        );
        assert_eq!(
            card_event(&Action::Removed, None, 1, true),
            CardEvent::Ignore
        );
    }

    #[test]
    fn a_connector_that_waited_for_a_crtc_is_brought_up_when_another_display_lets_one_go() {
        // three connectors, two CRTCs: `c` waits; unplugging `a` frees CRTC 1, which the scanner maps to `c`
        let driven = [(1, 'a'), (2, 'b')];
        assert_eq!(reconcile(&driven, &driven), (vec![], vec![]));
        assert_eq!(
            reconcile(&driven, &[(2, 'b'), (1, 'c')]),
            (vec![1], vec![(1, 'c')])
        );
        // a connector moved to another CRTC goes down on the old one and comes up on the new one
        assert_eq!(
            reconcile(&driven, &[(1, 'a'), (3, 'b')]),
            (vec![2], vec![(3, 'b')])
        );
        assert_eq!(reconcile(&driven, &[]), (vec![1, 2], vec![]));
    }

    #[test]
    fn only_a_drawn_scene_that_changed_nothing_is_reported_as_shown() {
        assert!(
            already_shown(true, false),
            "nothing differs from the screen"
        );
        assert!(
            !already_shown(true, true),
            "a new frame is queued, not shown yet"
        );
        assert!(
            !already_shown(false, false),
            "without a scene nothing says what the screen shows, so a lock is not confirmed by it"
        );
    }

    #[test]
    fn a_paused_session_renders_nothing_however_much_is_wanted() {
        assert!(frame_due(false, false, true));
        assert!(
            !frame_due(false, false, false),
            "the scheduler finds nothing due"
        );
        assert!(
            !frame_due(false, true, true),
            "a frame is still waiting for its vblank"
        );
        assert!(
            !frame_due(true, false, true),
            "the device rejects frames while paused"
        );
    }
}

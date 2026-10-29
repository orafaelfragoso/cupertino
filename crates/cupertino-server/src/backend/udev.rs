//! TTY backend: logind session, sysfs device discovery, one GPU (the primary one) with DRM/KMS
//! outputs (one head per connected connector, see `crate::displays`), evdev input devices (see `input.rs`). Single-GPU only (no multi-GPU copies).
//!
//! The render loop is driven by the main loop: an output is rendered when the compositor wants
//! frames and no frame is waiting for its vblank; the vblank event reports presentation.
//!
//! NOTE: written against the Smithay 0.7 API and compile-checked; it cannot run while another
//! session holds the DRM master (for example when started from a running desktop session).

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use cupertino_devices::{Action, Monitor, Subsystem, SEAT0};
use cupertino_render_api::{Completion as _, Failure, RenderFailure as _, RenderStatus, Renderer};
use cupertino_seat::LogindSession;
use cupertino_wm::OutputId;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::compositor::FrameError;
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements};
use smithay::backend::drm::{
    DrmDevice, DrmDeviceFd, DrmError, DrmEvent, DrmEventTime, DrmNode, NodeType,
};
use smithay::backend::egl::context::ContextPriority;
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::Color32F;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::desktop::utils::OutputPresentationFeedback;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, Mode, PostAction};
use smithay::reexports::drm::control::{
    self, connector, crtc, Device as ControlDevice, ModeFlags, ModeTypeFlags,
};
use smithay::reexports::drm::Device as DrmDeviceTrait;
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::utils::{DeviceFd, Transform};
use smithay::wayland::drm_syncobj::{supports_syncobj_eventfd, DrmSyncobjState};
use smithay::wayland::presentation::Refresh;
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};

use super::input;
use super::Backend;
use crate::displays::{edid, HeadCaps, HeadId, HeadInfo, HeadMode, HeadOp};
use crate::render::gles::{Gles, GlesResources, GlesTarget};
use crate::render::scene::RenderResources;
use crate::state::Cupertino;

const CLEAR: Color32F = Color32F::new(0.05, 0.05, 0.07, 1.0);
const COLOR_FORMATS: [Fourcc; 2] = [Fourcc::Abgr8888, Fourcc::Argb8888];

/// The DRM output of one display, as the renderer draws to it.
pub(crate) type DrmSurfaceOutput = DrmOutput<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    Option<OutputPresentationFeedback>,
    DrmDeviceFd,
>;
type Manager = DrmOutputManager<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    Option<OutputPresentationFeedback>,
    DrmDeviceFd,
>;

struct Surface {
    output: OutputId,
    drm_output: DrmSurfaceOutput,
    /// A frame was queued and its vblank has not arrived yet.
    pending: bool,
    refresh: Duration,
}

/// A connected display: its connector and the DRM modes behind its head's modes.
struct DrmHead {
    head: HeadId,
    connector: connector::Handle,
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

pub struct UdevBackend {
    session: LogindSession,
    renderer: GlesRenderer,
    manager: Manager,
    scanner: DrmScanner,
    /// The enabled heads' outputs.
    surfaces: HashMap<crtc::Handle, Surface>,
    /// Every connected head, by the CRTC the scanner gave its connector.
    heads: HashMap<crtc::Handle, DrmHead>,
    /// The command line's scale for every display.
    scale: Option<f64>,
    /// The session is paused: the DRM device rejects every frame until it resumes.
    paused: bool,
    /// The display sleeps (`power.rs`): every CRTC is off and nothing is rendered until it wakes.
    asleep: bool,
    /// A render reported the rendering context lost, which ends the session; nothing is drawn with it again.
    renderer_lost: bool,
}

/// Whether an output gets a frame now. A paused session or a lost renderer renders nothing: the device rejects the
/// frame, and the loop would retry (and log) every iteration. Nor does an output whose frame waits for its vblank or
/// for the retry time of a failed one.
fn frame_due(stopped: bool, waiting: bool, redraw: bool, wants_frames: bool) -> bool {
    !stopped && !waiting && (redraw || wants_frames)
}

impl UdevBackend {
    pub fn renderer(&mut self) -> &mut GlesRenderer {
        &mut self.renderer
    }

    pub fn renderer_lost(&self) -> bool {
        self.renderer_lost
    }

    /// Switch every CRTC off (DPMS off) or let the next frame switch it back on with a full redraw.
    pub fn set_asleep(&mut self, asleep: bool) {
        if self.asleep == asleep {
            return;
        }
        self.asleep = asleep;
        for s in self.surfaces.values_mut() {
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

/// Start the TTY backend and run until the compositor stops.
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
    let report = super::preflight::run(Path::new("/sys"))?;
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
    let gpu = cupertino_devices::primary_gpu(Path::new("/sys"))
        .map_err(|e| anyhow!("listing GPUs: {e}"))?
        .context("no GPU found")?;
    let gpu_path = gpu
        .device
        .devnode()
        .context("the GPU has no device file")?
        .to_owned();
    let gpu_devnum = gpu
        .device
        .devnum()
        .context("the GPU has no device number")?;
    tracing::info!(path = %gpu_path.display(), seat = %seat, "using GPU");

    let node = DrmNode::from_path(&gpu_path)
        .map_err(|e| anyhow!("drm node for {}: {e}", gpu_path.display()))?;
    let fd = session
        .clone()
        .open(
            &gpu_path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        )
        .map_err(|e| anyhow!("opening {}: {e}", gpu_path.display()))?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (drm, drm_notifier) =
        DrmDevice::new(fd.clone(), true).map_err(|e| anyhow!("drm device: {e}"))?;
    let import_fd = fd.clone();
    let gbm = GbmDevice::new(fd).map_err(|e| anyhow!("gbm device: {e} (the driver's GBM backend is missing or refused the device; run `cupertino-server --check-hardware`)"))?;

    let display = unsafe { EGLDisplay::new(gbm.clone()) }.map_err(|e| {
        anyhow!(
            "EGL display: {e} (no EGL platform for GBM; check the driver's EGL and GBM libraries)"
        )
    })?;
    let egl_device =
        EGLDevice::device_for_display(&display).map_err(|e| anyhow!("EGL device: {e}"))?;
    if egl_device.is_software() {
        return Err(anyhow!(
            "only a software renderer is available on {}",
            gpu_path.display()
        ));
    }
    let render_node = egl_device
        .try_get_render_node()
        .ok()
        .flatten()
        .or_else(|| node.node_with_type(NodeType::Render).and_then(|n| n.ok()));
    let context = EGLContext::new_with_priority(&display, ContextPriority::High)
        .map_err(|e| anyhow!("EGL context: {e}"))?;
    let mut renderer =
        unsafe { GlesRenderer::new(context) }.map_err(|e| anyhow!("GLES renderer: {e}"))?;
    let render_formats = renderer
        .egl_context()
        .dmabuf_render_formats()
        .iter()
        .copied()
        .collect::<smithay::backend::allocator::format::FormatSet>();

    state.render = Some(RenderResources::new(&mut renderer)?);
    super::init_buffers(state, &renderer);
    if supports_syncobj_eventfd(&import_fd) {
        let dh = state.display_handle.clone();
        state.syncobj_state = Some(DrmSyncobjState::new::<Cupertino>(&dh, import_fd));
        tracing::info!("explicit sync (linux-drm-syncobj-v1) is offered");
    } else {
        tracing::warn!("explicit sync is not offered: the kernel cannot wait on syncobj timelines");
    }

    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let exporter = GbmFramebufferExporter::new(gbm.clone(), render_node);
    let manager = DrmOutputManager::new(
        drm,
        allocator,
        exporter,
        Some(gbm),
        COLOR_FORMATS,
        render_formats,
    );

    let backend = UdevBackend {
        session: session.clone(),
        renderer,
        manager,
        scanner: DrmScanner::new(),
        surfaces: HashMap::new(),
        heads: HashMap::new(),
        scale,
        paused: false,
        asleep: false,
        renderer_lost: false,
    };
    state.backend = Some(Backend::Udev(Box::new(backend)));

    let handle = event_loop.handle();
    let inputs = input::new_inputs(handle.clone(), session.clone());

    let session_inputs = inputs.clone();
    handle
        .insert_source(notifier, move |event, &mut (), state| match event {
            SessionEvent::PauseSession => {
                tracing::info!("session paused");
                input::close_all(&session_inputs, state);
                if let Some(u) = udev(state) {
                    u.paused = true;
                    u.manager.pause();
                }
            }
            SessionEvent::ActivateSession => {
                tracing::info!("session resumed");
                if let Err(e) = input::open_all(&session_inputs, state) {
                    tracing::error!(error = %e, "reopening the input devices failed");
                }
                if let Some(u) = udev(state) {
                    u.paused = false;
                    if let Err(e) = u.manager.activate(false) {
                        tracing::error!(error = %e, "re-activating the DRM device failed");
                    }
                    for s in u.surfaces.values_mut() {
                        s.pending = false;
                    }
                }
                state.session_activated();
            }
        })
        .map_err(|e| anyhow!("inserting the session source: {e}"))?;

    handle
        .insert_source(drm_notifier, move |event, metadata, state| match event {
            DrmEvent::VBlank(crtc) => frame_finish(
                state,
                crtc,
                metadata.as_ref().map(|m| m.time),
                metadata.as_ref().map(|m| m.sequence),
            ),
            DrmEvent::Error(e) => tracing::error!(error = %e, "DRM error"),
        })
        .map_err(|e| anyhow!("inserting the DRM source: {e}"))?;

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
                            } else if event.subsystem == Subsystem::Drm.as_str()
                                && event.action == Action::Changed
                                && event.devnum == Some(gpu_devnum)
                            {
                                scan_connectors(state);
                            }
                        }
                        // Events were lost: the open devices may no longer match the nodes.
                        if drained.overflowed {
                            tracing::warn!(
                                "the uevent queue overflowed, enumerating the input devices again"
                            );
                            input::resync(&hotplug_inputs, state);
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
            .dispatch(Some(Duration::from_millis(4)), state)
            .map_err(|e| anyhow!("event loop: {e}"))?;
        state.tick();
        render_outputs(state);
        let _ = state.display_handle.flush_clients();
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
    }
    if let (Some(Backend::Udev(u)), Some(res)) = (state.backend.as_mut(), state.render.as_ref()) {
        res.destroy(&mut u.renderer);
    }
    Ok(())
}

/// Announce the heads of newly connected connectors (enabling them unless their stored state is
/// off) and disconnect the ones that went away.
fn scan_connectors(state: &mut Cupertino) {
    let Some(Backend::Udev(u)) = state.backend.as_mut() else {
        return;
    };
    let events: Vec<DrmScanEvent> = match u.scanner.scan_connectors(u.manager.device()) {
        Ok(result) => result.into_iter().collect(),
        Err(e) => {
            tracing::warn!(error = %e, "scanning connectors failed");
            return;
        }
    };
    for event in events {
        match event {
            DrmScanEvent::Connected {
                connector,
                crtc: Some(crtc),
            } => connector_connected(state, connector, crtc),
            DrmScanEvent::Disconnected {
                crtc: Some(crtc), ..
            } => {
                let Some(Backend::Udev(u)) = state.backend.as_mut() else {
                    return;
                };
                u.surfaces.remove(&crtc);
                if let Some(gone) = u.heads.remove(&crtc) {
                    state.disconnect_head(gone.head);
                }
            }
            _ => {}
        }
    }
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

fn connector_connected(state: &mut Cupertino, connector: connector::Info, crtc: crtc::Handle) {
    let Some(Backend::Udev(u)) = state.backend.as_mut() else {
        return;
    };
    let name = format!(
        "{}-{}",
        connector.interface().as_str(),
        connector.interface_id()
    );
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
    let identity =
        connector_edid(u.manager.device(), connector.handle()).and_then(|blob| edid::parse(&blob));
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
    let scale = u.scale;
    let (head, initial) = state.connect_head(info, scale);
    let Some(Backend::Udev(u)) = state.backend.as_mut() else {
        return;
    };
    u.heads.insert(
        crtc,
        DrmHead {
            head,
            connector: connector.handle(),
            modes,
        },
    );
    if initial.enabled {
        if let Err(e) = state.head_op(head, HeadOp::Enable) {
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
    let (crtc, drm_head) = u
        .heads
        .iter()
        .find(|(_, h)| h.head == head)
        .map(|(c, h)| (*c, h))
        .context("the head has no connector")?;
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
            let id = state.create_head_output(head);
            let output = state
                .output_entry(id)
                .map(|o| o.output.clone())
                .context("the output just created")?;
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
                    let refresh =
                        Duration::from_secs_f64(1000.0 / mode.refresh_mhz.max(1000) as f64);
                    u.surfaces.insert(
                        crtc,
                        Surface {
                            output: id,
                            drm_output,
                            pending: false,
                            refresh,
                        },
                    );
                    tracing::info!(output = %output.name(), width = mode.width, height = mode.height, refresh_mhz = mode.refresh_mhz, "output initialised");
                    Ok(())
                }
                Err(e) => {
                    state.drop_head_output(head);
                    Err(anyhow!("initialising {}: {e}", output.name()))
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
            let elements =
                DrmOutputRenderElements::<GlesRenderer, crate::render::CupertinoElement>::default();
            surface
                .drm_output
                .use_mode(drm_mode, &mut u.renderer, &elements)
                .map_err(|e| anyhow!("switching the mode: {e}"))?;
            surface.refresh = Duration::from_secs_f64(1000.0 / mode.refresh_mhz.max(1000) as f64);
            surface.pending = false;
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
            }
            Ok(())
        }
    }
}

fn render_outputs(state: &mut Cupertino) {
    let Some(mut backend) = state.backend.take() else {
        return;
    };
    if let Backend::Udev(u) = &mut backend {
        let crtcs: Vec<crtc::Handle> = u.surfaces.keys().copied().collect();
        let now = Duration::from(state.clock.now());
        for crtc in crtcs {
            let Some(s) = u.surfaces.get(&crtc) else {
                continue;
            };
            let Some(idx) = state.outputs.iter().position(|o| o.id == s.output) else {
                continue;
            };
            if !state.displays.powered(s.output)
                || !frame_due(
                    u.paused || u.asleep || u.renderer_lost,
                    s.pending || !state.retry_due(idx, now),
                    state.outputs[idx].scheduler.damaged(),
                    state.wants_frames(),
                )
            {
                continue;
            }
            render_surface(state, u, crtc, idx);
        }
    }
    state.backend = Some(backend);
}

/// Whether the frame already on the screen is what this frame would show, so it may be reported as shown: only when a
/// scene was composed and drawn and nothing differed from it. Without a scene nothing says what the screen shows, and
/// the session lock must not be confirmed by it.
fn already_shown(scene: bool, rendered: bool) -> bool {
    scene && !rendered
}

fn render_surface(state: &mut Cupertino, u: &mut UdevBackend, crtc: crtc::Handle, idx: usize) {
    // without a scene (no render resources or no mode yet) or a drawn frame, nothing is queued, but the tail still runs
    let mut scene = true;
    let mut failed = None;
    let rendered = match state.compose_for_present(&mut u.renderer, idx) {
        None => {
            scene = false;
            false
        }
        Some(composed) => {
            let Some(surface) = u.surfaces.get_mut(&crtc) else {
                return;
            };
            let Some((res, cache, _)) = state.gles_parts(idx) else {
                return;
            };
            let mut resources = GlesResources::new(res, cache, composed.resources);
            let result = Gles.render(
                &composed.frame,
                &mut resources,
                GlesTarget::Drm {
                    renderer: &mut u.renderer,
                    output: &mut surface.drm_output,
                    clear: CLEAR,
                },
            );
            state.outputs[idx].scheduler.rendered();
            match result {
                Ok(completion) => {
                    state.frame_drawn(idx);
                    let rendered = completion.status() == RenderStatus::Rendered;
                    if !rendered {
                        state.outputs[idx].scheduler.empty();
                    }
                    rendered
                }
                Err(e) => {
                    u.renderer_lost |= e.failure() == Failure::ContextLost;
                    failed = Some(e);
                    scene = false;
                    false
                }
            }
        }
    };
    let Some(surface) = u.surfaces.get_mut(&crtc) else {
        return;
    };
    let time = Duration::from(state.clock.now());
    if rendered {
        let feedback = state.collect_feedback(idx);
        match surface.drm_output.queue_frame(Some(feedback)) {
            Ok(()) => {
                surface.pending = true;
                state.outputs[idx].scheduler.queued();
            }
            // logind drops DRM master before it sends the pause, so a flip in between is refused; the pause follows
            Err(FrameError::DrmError(DrmError::Access(e)))
                if e.source.kind() == ErrorKind::PermissionDenied =>
            {
                tracing::debug!(error = %e, "the page flip was refused: the session is being paused")
            }
            Err(e) => tracing::warn!(error = %e, "queueing the frame failed"),
        }
    }
    if already_shown(scene, rendered) {
        state.note_frame_shown(idx);
    }
    state.process_screenshots(&mut u.renderer, idx);
    match failed {
        Some(e) => state.frame_failed(idx, time, surface.refresh, &e),
        None => state.send_frames(idx, time),
    }
}

fn frame_finish(
    state: &mut Cupertino,
    crtc: crtc::Handle,
    time: Option<DrmEventTime>,
    seq: Option<u32>,
) {
    let (clock, flags) = match time {
        Some(DrmEventTime::Monotonic(t)) if !t.is_zero() => (
            t.into(),
            wp_presentation_feedback::Kind::Vsync
                | wp_presentation_feedback::Kind::HwClock
                | wp_presentation_feedback::Kind::HwCompletion,
        ),
        _ => (state.clock.now(), wp_presentation_feedback::Kind::Vsync),
    };
    let Some(u) = udev(state) else { return };
    let Some(surface) = u.surfaces.get_mut(&crtc) else {
        return;
    };
    surface.pending = false;
    let output = surface.output;
    match surface.drm_output.frame_submitted() {
        Ok(Some(Some(mut feedback))) => feedback.presented(
            clock,
            Refresh::fixed(surface.refresh),
            seq.unwrap_or(0) as u64,
            flags,
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "frame submission failed"),
    }
    if let Some(idx) = state.outputs.iter().position(|o| o.id == output) {
        state.note_frame_shown(idx);
    }
    state.request_redraw();
}

#[cfg(test)]
mod tests {
    use super::{already_shown, frame_due};

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
        assert!(frame_due(false, false, true, false));
        assert!(frame_due(false, false, false, true));
        assert!(!frame_due(false, false, false, false), "nothing changed");
        assert!(
            !frame_due(false, true, true, true),
            "a frame is still waiting for its vblank"
        );
        assert!(
            !frame_due(true, false, true, true),
            "the device rejects frames while paused"
        );
    }
}

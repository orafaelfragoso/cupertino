//! Headless backend: an EGL device without a window system renders into an offscreen texture.
//! Used by `--headless`, the integration tests and screenshots without a display.
//!
//! Device selection order: `$CUPERTINO_RENDER_NODE` (GBM on that node), the first EGL device with
//! a render node, the surfaceless platform, the default render node `/dev/dri/renderD128`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use cupertino_render_api::{Completion as _, RenderStatus, Renderer};
use cupertino_wm::OutputId;
use smithay::backend::allocator::gbm::GbmDevice;
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::DrmDeviceFd;
use smithay::backend::egl::native::EGLSurfacelessDisplay;
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Color32F, Offscreen};
use smithay::reexports::calloop::EventLoop;
use smithay::utils::{Buffer, DeviceFd, Size, Transform};
use smithay::wayland::drm_syncobj::{supports_syncobj_eventfd, DrmSyncobjState};

use super::Backend;
use crate::displays::{HeadCaps, HeadId, HeadInfo, HeadMode, HeadOp};
use crate::render::gles::{Gles, GlesResources, GlesTarget};
use crate::render::scene::RenderResources;
use crate::state::Cupertino;

/// What the error says when no EGL device can render the output.
pub const NO_USABLE_DEVICE: &str = "no usable EGL device";

pub const CLEAR: Color32F = Color32F::new(0.05, 0.05, 0.07, 1.0);

/// One offscreen output: the texture it is drawn into.
pub struct HeadlessTarget {
    pub head: HeadId,
    pub output: OutputId,
    pub texture: GlesTexture,
}

pub struct HeadlessBackend {
    pub renderer: GlesRenderer,
    pub targets: Vec<HeadlessTarget>,
    /// Modes enabling a head in fails, with the error kind the device gives: `InvalidInput` as over a link too slow for
    /// the mode, `ResourceBusy` as when another program holds the device.
    pub refused: Vec<(HeadMode, std::io::ErrorKind)>,
}

/// An offscreen display to plug in: its connector name, pixel size and physical size in
/// millimetres (zero when it has none, which keeps it unscaled).
#[derive(Clone, Debug)]
pub struct HeadlessDisplay {
    pub name: String,
    pub size: (i32, i32),
    pub physical_mm: (i32, i32),
}

fn renderer_from_display(display: &EGLDisplay) -> Result<GlesRenderer> {
    let context = EGLContext::new_with_reset_notification(display, None)
        .map_err(|e| anyhow!("creating the EGL context: {e}"))?;
    // Safety: the context was created for this display and is only used by this renderer.
    unsafe { super::gles_renderer(context) }.map_err(|e| anyhow!("creating the GLES renderer: {e}"))
}

fn from_gbm(path: &str) -> Result<GlesRenderer> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("opening {path}"))?;
    let gbm = GbmDevice::new(file).map_err(|e| anyhow!("gbm device on {path}: {e}"))?;
    let display =
        unsafe { EGLDisplay::new(gbm) }.map_err(|e| anyhow!("EGL display on {path}: {e}"))?;
    renderer_from_display(&display)
}

/// Create an offscreen GLES renderer, trying the strategies described in the module docs.
pub fn create_renderer() -> Result<GlesRenderer> {
    let mut errors = Vec::new();
    if let Some(node) = std::env::var_os("CUPERTINO_RENDER_NODE").map(PathBuf::from) {
        match from_gbm(&node.to_string_lossy()) {
            Ok(r) => return Ok(r),
            Err(e) => errors.push(format!("{}: {e:#}", node.display())),
        }
    }
    if let Ok(devices) = EGLDevice::enumerate() {
        for dev in devices {
            if dev.try_get_render_node().ok().flatten().is_none() {
                continue;
            }
            match unsafe { EGLDisplay::new(dev) }
                .map_err(|e| anyhow!("{e}"))
                .and_then(|d| renderer_from_display(&d))
            {
                Ok(r) => return Ok(r),
                Err(e) => errors.push(format!("EGL device: {e:#}")),
            }
        }
    }
    match unsafe { EGLDisplay::new(EGLSurfacelessDisplay) }
        .map_err(|e| anyhow!("{e}"))
        .and_then(|d| renderer_from_display(&d))
    {
        Ok(r) => return Ok(r),
        Err(e) => errors.push(format!("surfaceless: {e:#}")),
    }
    match from_gbm("/dev/dri/renderD128") {
        Ok(r) => Ok(r),
        Err(e) => {
            errors.push(format!("renderD128: {e:#}"));
            Err(anyhow!("{NO_USABLE_DEVICE}: {}", errors.join("; ")))
        }
    }
}

/// Offer `linux-drm-syncobj-v1` on the render node of `renderer`'s device when it has one whose kernel waits on
/// timeline points through an eventfd; clients' acquire points are then waited for and release points signalled as on
/// a display. A render node can never hold the device's master, so opening it takes nothing from a session on the
/// same device.
fn offer_explicit_sync(state: &mut Cupertino, renderer: &GlesRenderer) {
    let Some(path) = super::render_node(renderer).and_then(|node| node.dev_path()) else {
        return;
    };
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "explicit sync is not offered: the render node cannot be opened");
            return;
        }
    };
    let fd = DrmDeviceFd::new(DeviceFd::from(std::os::fd::OwnedFd::from(file)));
    if supports_syncobj_eventfd(&fd) {
        let dh = state.display_handle.clone();
        state.syncobj_state = Some(DrmSyncobjState::new::<Cupertino>(&dh, fd));
        tracing::info!(path = %path.display(), "explicit sync (linux-drm-syncobj-v1) is offered");
    } else {
        tracing::info!("explicit sync is not offered: the kernel cannot wait on syncobj timelines");
    }
}

/// Create the offscreen renderer and one display of `size` pixels at `scale`.
pub fn init(state: &mut Cupertino, size: (i32, i32), scale: f64) -> Result<()> {
    init_displays(
        state,
        &[HeadlessDisplay {
            name: "headless-1".into(),
            size,
            physical_mm: (0, 0),
        }],
        Some(scale),
    )
}

/// Create the offscreen renderer and plug in `displays`; `scale` beats their stored and default scales.
pub fn init_displays(
    state: &mut Cupertino,
    displays: &[HeadlessDisplay],
    scale: Option<f64>,
) -> Result<()> {
    let mut renderer = create_renderer()?;
    cupertino_diagnostic_events::set_context(
        "renderer",
        cupertino_diagnostic_events::Value::Public("gles"),
    );
    state.render = Some(RenderResources::new(&mut renderer)?);
    super::init_buffers(state, &renderer);
    if !state.withhold_explicit_sync {
        offer_explicit_sync(state, &renderer);
    }
    state.backend = Some(Backend::Headless(Box::new(HeadlessBackend {
        renderer,
        targets: Vec::new(),
        refused: Vec::new(),
    })));
    for display in displays {
        add_display(state, display, scale)?;
    }
    Ok(())
}

/// Plug in an offscreen display (hot-plug): its head is announced and, unless its stored state is
/// off, its output created.
pub fn add_display(
    state: &mut Cupertino,
    display: &HeadlessDisplay,
    scale: Option<f64>,
) -> Result<HeadId> {
    add_display_with_modes(state, display, &[], scale)
}

/// [`add_display`] for a display that offers `more` modes after its own size at 60 Hz.
pub fn add_display_with_modes(
    state: &mut Cupertino,
    display: &HeadlessDisplay,
    more: &[HeadMode],
    scale: Option<f64>,
) -> Result<HeadId> {
    add_display_at(state, display, DEFAULT_REFRESH_MHZ, more, scale)
}

/// The refresh rate of an offscreen display's own mode unless one is given, in mHz.
pub const DEFAULT_REFRESH_MHZ: i32 = 60_000;

/// [`add_display_with_modes`] for a display whose own mode refreshes at `refresh_mhz`: its frames are paced, and their
/// presentation reported, at that rate.
pub fn add_display_at(
    state: &mut Cupertino,
    display: &HeadlessDisplay,
    refresh_mhz: i32,
    more: &[HeadMode],
    scale: Option<f64>,
) -> Result<HeadId> {
    let mut modes = vec![HeadMode {
        width: display.size.0,
        height: display.size.1,
        refresh_mhz,
    }];
    modes.extend_from_slice(more);
    let info = HeadInfo {
        connector: display.name.clone(),
        make: "Cupertino".into(),
        model: display.name.clone(),
        serial: None,
        physical_mm: display.physical_mm,
        modes,
        caps: HeadCaps {
            can_disable: true,
            custom_modes: true,
            transforms: true,
        },
        base_transform: Transform::Normal,
    };
    let (head, initial) = state.connect_head(info, scale);
    if initial.enabled {
        state.enable_head(head)?;
        state.heads_changed();
    }
    Ok(head)
}

/// Unplug an offscreen display.
pub fn remove_display(state: &mut Cupertino, head: HeadId) -> Result<()> {
    if state
        .displays
        .head(head)
        .is_some_and(|h| h.output.is_some())
    {
        state.head_op(head, HeadOp::Disable)?;
    }
    state.disconnect_head(head);
    Ok(())
}

/// What the udev backend does after a GPU reset ([`crate::gpu_reset`]), for tests: a new context and renderer on the same
/// EGL display, new output textures and every GPU object of the scene made again. The old renderer is dropped, so
/// nothing drawn with it survives.
pub fn simulate_gpu_reset(state: &mut Cupertino) -> Result<()> {
    let Some(Backend::Headless(h)) = state.backend.as_mut() else {
        bail!("not the headless backend");
    };
    let display = h.renderer.egl_context().display().clone();
    h.renderer = renderer_from_display(&display)?;
    for target in &mut h.targets {
        let mode = state
            .displays
            .head(target.head)
            .context("the head of an output")?
            .state
            .mode;
        target.texture = target_texture(&mut h.renderer, mode)?;
    }
    let Some(Backend::Headless(mut h)) = state.backend.take() else {
        unreachable!("checked above");
    };
    let rebuilt = state.rebuild_render(&mut h.renderer);
    state.backend = Some(Backend::Headless(h));
    rebuilt
}

fn target_texture(renderer: &mut GlesRenderer, mode: HeadMode) -> Result<GlesTexture> {
    let size: Size<i32, Buffer> = (mode.width, mode.height).into();
    renderer
        .create_buffer(Fourcc::Abgr8888, size)
        .map_err(|e| anyhow!("creating the output texture: {e}"))
}

/// Carry out `op` on an offscreen head. Power changes need nothing: an output that is off is
/// skipped when rendering.
pub(crate) fn head_op(
    state: &mut Cupertino,
    h: &mut HeadlessBackend,
    head: HeadId,
    op: HeadOp,
) -> Result<()> {
    match op {
        HeadOp::Enable => {
            let mode = state
                .displays
                .head(head)
                .context("the head being enabled")?
                .state
                .mode;
            let output = state.prepare_head_output(head)?;
            if let Some((_, kind)) = h.refused.iter().find(|(m, _)| *m == mode) {
                return Err(anyhow::Error::new(std::io::Error::from(*kind))
                    .context(format!("the display cannot show {mode:?}")));
            }
            let texture = target_texture(&mut h.renderer, mode)?;
            let output = state.create_head_output(head, output);
            h.targets.push(HeadlessTarget {
                head,
                output,
                texture,
            });
        }
        HeadOp::Disable => {
            h.targets.retain(|t| t.head != head);
            state.drop_head_output(head);
        }
        HeadOp::SetMode(mode) => {
            if let Some((_, kind)) = h.refused.iter().find(|(m, _)| *m == mode) {
                return Err(anyhow::Error::new(std::io::Error::from(*kind))
                    .context(format!("the display cannot show {mode:?}")));
            }
            let texture = target_texture(&mut h.renderer, mode)?;
            h.targets
                .iter_mut()
                .find(|t| t.head == head)
                .context("the head is not enabled")?
                .texture = texture;
        }
        HeadOp::Power(_) => {}
    }
    Ok(())
}

/// Render a frame of every powered output that is due into its texture and send frame callbacks.
#[allow(irrefutable_let_patterns)]
pub fn render(state: &mut Cupertino) {
    let Some(mut backend) = state.backend.take() else {
        return;
    };
    if let Backend::Headless(h) = &mut backend {
        for target in &mut h.targets {
            let Some(idx) = state.outputs.iter().position(|o| o.id == target.output) else {
                continue;
            };
            // no vblank paces an offscreen output; a fixed clock is stepped, and so paced, by its owner
            let paced = state.fixed_time.is_none();
            state.outputs[idx].scheduler.pace_changes(paced);
            if !state.displays.powered(target.output) || !state.frame_due(idx, state.frame_now()) {
                continue;
            }
            // without a scene (no render resources or no mode yet) nothing is drawn, but callbacks still go out
            let composing = Instant::now();
            let timing = state.frame_stats.is_some();
            if timing {
                crate::frame_stats::start_glass_timing(state);
            }
            if let Some(composed) = state.compose_for_present(&mut h.renderer, idx) {
                let compose = composing.elapsed();
                let rendering = Instant::now();
                if let Some(timer) = state.frame_stats.as_ref().and_then(|s| s.timer.as_ref()) {
                    let _ = h.renderer.with_context(|_| unsafe { timer.stamp(false) });
                }
                if let Some((res, cache, tracker)) = state.gles_parts(idx) {
                    let mut resources = GlesResources::new(res, cache, composed.resources);
                    let result = Gles.render(
                        &composed.frame,
                        &mut resources,
                        GlesTarget::Texture {
                            renderer: &mut h.renderer,
                            texture: &mut target.texture,
                            tracker,
                            age: 0,
                            clear: CLEAR,
                        },
                    );
                    let scheduler = &mut state.outputs[idx].scheduler;
                    scheduler.rendered();
                    match result {
                        Ok(c) if c.status() != RenderStatus::Rendered => scheduler.empty(),
                        Ok(_) => scheduler.queued(),
                        Err(e) => tracing::warn!(error = %e, "headless render failed"),
                    }
                }
                if timing {
                    // the GPU's work belongs to the frame's time: wait for it
                    let st = &*state;
                    let timer = st.frame_stats.as_ref().and_then(|s| s.timer.as_ref());
                    let gpu = h
                        .renderer
                        .with_context(|gl| unsafe {
                            if let Some(timer) = timer {
                                timer.stamp(true);
                            }
                            gl.Finish();
                            crate::frame_stats::collect_pass_times(st, gl);
                            timer.map(|t| t.elapsed())
                        })
                        .ok()
                        .flatten();
                    crate::frame_stats::frame_rendered(
                        state,
                        idx,
                        compose,
                        rendering.elapsed(),
                        gpu,
                    );
                }
            }
            state.process_screenshots(&mut h.renderer, idx);
            let time = Duration::from(state.clock.now());
            let refresh = state.refresh_of(idx);
            state.post_repaint(idx, time, Some(refresh), true);
        }
    }
    state.backend = Some(backend);
}

/// One loop iteration: dispatch events, advance the model, render when needed.
pub fn step(
    state: &mut Cupertino,
    event_loop: &mut EventLoop<'static, Cupertino>,
    timeout: Duration,
) -> Result<()> {
    event_loop
        .dispatch(Some(timeout), state)
        .map_err(|e| anyhow!("event loop: {e}"))?;
    state.tick();
    let now = state.frame_now();
    crate::frame_stats::note_owed(state, now);
    if state.wants_frames() {
        render(state);
    }
    let _ = state.display_handle.flush_clients();
    Ok(())
}

/// The longest a headless loop waits without an event: until the earliest output is due to draw (`frames`), the
/// next time-driven work of [`Cupertino::tick`] (`tick`) or the end of the run (`end`), and at most
/// [`super::REST_WAIT`]. With nothing due the loop sleeps; clients, input and the scripted steps' timers wake it sooner.
fn loop_wait(frames: Option<Duration>, tick: Option<Duration>, end: Option<Duration>) -> Duration {
    super::loop_wait([frames, tick, end])
}

/// Run until `deadline` or until the compositor stops.
#[allow(irrefutable_let_patterns)]
pub fn run(
    state: &mut Cupertino,
    event_loop: &mut EventLoop<'static, Cupertino>,
    deadline: Option<Instant>,
) -> Result<()> {
    while state.running {
        let now = state.frame_now();
        let frames = (0..state.outputs.len())
            .filter(|&idx| state.displays.powered(state.outputs[idx].id))
            .filter_map(|idx| state.frame_deadline(idx, now))
            .min()
            .map(|at| at.saturating_sub(now));
        let end = deadline.map(|d| d.saturating_duration_since(Instant::now()));
        let timeout = loop_wait(frames, super::tick_wait(state), end);
        step(state, event_loop, timeout)?;
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
    }
    if let Some(Backend::Headless(h)) = state.backend.as_mut() {
        if let Some(res) = state.render.as_ref() {
            res.destroy(&mut h.renderer);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn a_session_at_rest_sleeps_and_wakes_its_loop_once_a_minute_at_most() {
        assert_eq!(loop_wait(None, None, None), super::super::REST_WAIT);
        // a minute of rest is one pass, not the sixty a one-second backstop makes
        let passes = Duration::from_secs(60).as_millis() / loop_wait(None, None, None).as_millis();
        assert_eq!(passes, 1);
    }

    #[test]
    fn the_loop_wakes_for_the_first_of_a_frame_a_tick_and_the_end_of_the_run() {
        assert_eq!(loop_wait(Some(16 * MS), Some(200 * MS), None), 16 * MS);
        assert_eq!(loop_wait(None, Some(200 * MS), Some(3000 * MS)), 200 * MS);
        assert_eq!(loop_wait(None, None, Some(300 * MS)), 300 * MS);
        assert_eq!(loop_wait(Some(5000 * MS), None, None), 5000 * MS);
        assert_eq!(
            loop_wait(Some(Duration::from_secs(600)), None, None),
            super::super::REST_WAIT
        );
        assert_eq!(loop_wait(Some(Duration::ZERO), None, None), Duration::ZERO);
    }
}

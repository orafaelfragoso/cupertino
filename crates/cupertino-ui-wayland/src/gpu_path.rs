//! The GPU path of a window system: deciding its renderer by the fallback rule ([`crate::renderer`]), asking the
//! process's render thread ([`render_thread`]) for its device while the first frames are drawn on the CPU, switching to
//! the GPU once the device is ready (the windows' renderers then draw on the render thread, [`GpuRenderers`]), the GPU
//! buffers of each surface, falling back per surface to shared memory when a buffer cannot be made or imported, and
//! recovering from a lost context (three resets within a minute leave the window system on the CPU; purges never
//! count).

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cupertino_gpu::syncobj::SyncNode;
use cupertino_gpu::{program_cache, Loss, FOURCC_ARGB8888};
use cupertino_ui_core::platform::PlatformEvent;
use cupertino_ui_render::gpu::{GpuConfig, GpuRenderer, WindowRenderer};
use cupertino_ui_render::render_thread::{
    self, Claim, Ready, Resident, Status, WarmTiming, DEVICE_HANG,
};
use cupertino_ui_render::CpuFrame;
use cupertino_ui_text::GlyphRasterizer;
use cupertino_wayland_client::{Connection, PixelRect, SurfaceId, Waker};

use crate::gpu_present::{Draw, GpuDevice, GpuSurface, PresentError, SurfaceStats};
use crate::renderer::{
    self, Busy, Choice, FallbackScope, Feedback, LossHistory, Reason, RendererClass, RendererKind,
    Requested, Start,
};

/// The render node of the device `dev` names (a `dev_t` from dmabuf feedback): the node itself when it is one, else
/// the render node of the same device; `None` when the process cannot see one (a sandbox without `/dev/dri`).
pub fn render_node_of(dev: u64) -> Option<PathBuf> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let nodes = std::fs::read_dir("/dev/dri").ok()?;
    for entry in nodes.flatten() {
        let path = entry.path();
        let is_render = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("renderD"));
        if is_render
            && std::fs::metadata(&path)
                .is_ok_and(|m| m.file_type().is_char_device() && m.rdev() == dev)
        {
            return Some(path);
        }
    }
    // a primary node: its device's render node, through sysfs
    let (major, minor) = (libc_major(dev), libc_minor(dev));
    let drm = PathBuf::from(format!("/sys/dev/char/{major}:{minor}/device/drm"));
    std::fs::read_dir(drm)
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .find(|n| n.starts_with("renderD"))
        .map(|n| PathBuf::from("/dev/dri").join(n))
        // a sandbox may hide the node the kernel has
        .filter(|node| node.exists())
}

/// The major number of a Linux `dev_t`.
fn libc_major(dev: u64) -> u64 {
    ((dev >> 32) & 0xffff_f000) | ((dev >> 8) & 0x0000_0fff)
}

/// The minor number of a Linux `dev_t`.
fn libc_minor(dev: u64) -> u64 {
    ((dev >> 12) & 0xffff_ff00) | (dev & 0x0000_00ff)
}

/// What makes the windows' renderers on the process's GPU: which of the process's devices the window system draws on
/// while it draws on the GPU, nothing otherwise. A renderer factory holds a clone and asks it for a renderer each time
/// it makes one, so the renderers made after a switch ([`PlatformEvent::RendererChanged`]) draw on the device the
/// frames are presented from.
#[derive(Clone, Default)]
pub struct GpuRenderers(Rc<Cell<Option<u64>>>);

impl GpuRenderers {
    /// A window's renderer on the process's render thread, or `None` while the window system draws on the CPU.
    pub fn renderer<R: GlyphRasterizer + Send + 'static>(
        &self,
        config: GpuConfig,
        rasterizer: R,
    ) -> Option<WindowRenderer<R>> {
        let generation = self.0.get()?;
        render_thread::run(move |on| {
            let device = on.device_of(generation).ok()?;
            let programs = on.programs()?;
            let renderer = GpuRenderer::with_programs(
                Rc::clone(device),
                Rc::clone(programs),
                config,
                rasterizer,
            );
            Some(Resident::new(on, renderer))
        })
        .map(WindowRenderer::new)
    }

    /// Whether renderers are made on the GPU now.
    pub fn on(&self) -> bool {
        self.0.get().is_some()
    }

    fn set(&self, on: Option<u64>) {
        self.0.set(on);
    }
}

enum State {
    /// The window system draws on the CPU.
    Cpu,
    /// The device is being made; frames are drawn on the CPU meanwhile.
    Warming,
    On(Box<GpuDevice>),
}

/// What a surface presents through.
enum SurfaceKind {
    Gpu(GpuSurface),
    /// A buffer could not be made or imported: shared memory from now on.
    Fallback,
}

/// What [`GpuPath::present`] did.
pub enum Presented {
    /// Shown from a GPU buffer.
    Gpu,
    /// Not for the GPU (the window system or the surface draws into shared memory): present it there.
    Shm,
    /// Every buffer is the compositor's still.
    Busy,
    /// The frame is not of the size the surface asks for now.
    Resized,
}

/// What [`GpuPath::prepare`] did.
pub enum Prepared {
    /// A buffer is ready for the frame: draw it there and commit.
    Gpu(Box<Draw>),
    /// Not for the GPU: present it in shared memory.
    Shm,
    /// Every buffer is the compositor's still.
    Busy,
    /// The frame is not of the size the surface asks for now.
    Resized,
}

/// A window system's GPU path.
pub struct GpuPath {
    state: State,
    requested: Requested,
    class: RendererClass,
    /// The rule's class before any window was large or busy: each window's presents counted, no device asked for and
    /// the dmabuf protocol not bound.
    waiting: Option<HashMap<SurfaceId, Busy>>,
    node: Option<PathBuf>,
    offered: Vec<u64>,
    explicit_offered: bool,
    /// The context losses counted toward staying on the CPU.
    losses: LossHistory,
    /// The renderer it draws with now, and why; `None` until the rule first decided.
    choice: Option<Choice>,
    surfaces: HashMap<SurfaceId, SurfaceKind>,
    cache: Option<PathBuf>,
    /// Its hold on the process's device, once it asked for one, and when it asked; dropped after `state` and
    /// `surfaces`, so what they keep on the render thread goes there before the claim lets the device go.
    claim: Option<(Claim, Instant)>,
    /// Wakes the window system's loop once the device it asked for is ready.
    waker: Waker,
    /// The warm-up's timing and when the switch happened after the request, once the GPU took over.
    pub warmed: Option<(WarmTiming, Duration)>,
    /// Events for the runtime: a change of renderer.
    events: Vec<PlatformEvent>,
    renderers: GpuRenderers,
    /// A claim given up while the warm-up hung in `Device::new`, kept until the window system goes.
    abandoned: Option<Claim>,
}

impl GpuPath {
    /// Decide by the process's class and the rule (`CUPERTINO_RENDERER` overriding the class) and ask for the device
    /// when the GPU may be used.
    pub fn start(conn: &mut Connection, class: RendererClass) -> GpuPath {
        Self::start_with(conn, Requested::from_env(), class)
    }

    pub fn start_with(
        conn: &mut Connection,
        requested: Requested,
        class: RendererClass,
    ) -> GpuPath {
        let mut path = GpuPath {
            state: State::Cpu,
            requested,
            class,
            waiting: None,
            node: None,
            offered: Vec::new(),
            explicit_offered: false,
            losses: LossHistory::default(),
            choice: None,
            surfaces: HashMap::new(),
            cache: program_cache::default_dir(),
            claim: None,
            waker: conn.waker(),
            warmed: None,
            events: Vec::new(),
            renderers: GpuRenderers::default(),
            abandoned: None,
        };
        let wanted = renderer::on_at_start(class);
        if requested == Requested::Default && class == RendererClass::ByUse {
            path.waiting = Some(HashMap::new());
        }
        path.decide(conn, wanted);
        path
    }

    /// Run the rule's first half with `wanted` (whether the class wants the GPU now) and ask for the device if it says
    /// so. A window system that draws on the CPU never binds the dmabuf protocol.
    fn decide(&mut self, conn: &mut Connection, wanted: bool) {
        let wants_device = match self.requested {
            Requested::Cpu => false,
            Requested::Default => wanted && self.class != RendererClass::Cpu,
            Requested::Gpu => true,
        };
        let feedback = if wants_device {
            conn.dmabuf_feedback().ok().flatten()
        } else {
            None
        };
        self.offered = feedback
            .as_ref()
            .map(|f| f.modifiers(FOURCC_ARGB8888))
            .unwrap_or_default();
        self.explicit_offered = wants_device && conn.has_explicit_sync().unwrap_or(false);
        let feedback = match feedback {
            None => Feedback::None,
            Some(f) => match f.main_device {
                None => Feedback::NoDevice,
                Some(dev) => render_node_of(dev).map_or(Feedback::NoNode, Feedback::Node),
            },
        };
        match renderer::start(self.requested, self.class, wanted, feedback) {
            Start::Cpu(reason) => self.choose(Choice {
                kind: RendererKind::Cpu,
                reason,
            }),
            Start::Device { node } => {
                self.node = node;
                self.begin_warm();
            }
        }
    }

    /// The process's class.
    pub fn class(&self) -> RendererClass {
        self.class
    }

    /// The renderer it draws with now, and why: the CPU until the rule decided otherwise.
    pub fn choice(&self) -> Choice {
        self.choice.clone().unwrap_or(Choice {
            kind: RendererKind::Cpu,
            reason: Reason::CpuClass,
        })
    }

    /// Takes `choice`, announced once per change ([`renderer::announce`]).
    fn choose(&mut self, choice: Choice) {
        if self.choice.as_ref() != Some(&choice) {
            renderer::announce(&choice);
            self.choice = Some(choice);
        }
    }

    /// Whether the process takes the size and repaint rule and no window of it was large or busy yet.
    pub fn waiting(&self) -> bool {
        self.waiting.is_some()
    }

    /// `surface` presented a frame of `size` pixels at `at_ns` (monotonic nanoseconds): while the process waits for
    /// the rule, a window larger than a quarter of the largest output it is on (any output before it entered one) or
    /// busy for two seconds moves the window system to the GPU, its first frames there still the CPU's while the
    /// device is made.
    pub fn presented(
        &mut self,
        conn: &mut Connection,
        surface: SurfaceId,
        size: (u32, u32),
        at_ns: u64,
    ) {
        let Some(waiting) = self.waiting.as_mut() else {
            return;
        };
        let busy = waiting.entry(surface).or_default().presented(at_ns);
        let entered = conn.surface_outputs(surface).unwrap_or_default();
        let output = conn
            .outputs()
            .into_iter()
            .filter(|o| entered.is_empty() || entered.contains(&o.id))
            .filter_map(|o| o.mode)
            .max_by_key(|&(w, h)| u64::from(w) * u64::from(h));
        let large = output.is_some_and(|output| renderer::large(size, output));
        if large || busy {
            self.waiting = None;
            let why = if large {
                "covers more than a quarter of its output".to_string()
            } else {
                format!(
                    "presented more than {} frames a second for {} s",
                    renderer::BUSY_FRAMES_PER_SECOND,
                    renderer::BUSY_SECONDS
                )
            };
            eprintln!("renderer: a window {why}: the process moves to the GPU");
            self.decide(conn, true);
        }
    }

    /// Ask the render thread for the process's device; the window system's loop is woken once it is ready.
    fn begin_warm(&mut self) {
        let waker = self.waker.clone();
        let claim =
            render_thread::request(self.node.as_deref(), self.cache.as_deref(), move || {
                waker.wake()
            });
        self.claim = Some((claim, Instant::now()));
        self.state = State::Warming;
        self.choose(Choice {
            kind: RendererKind::Cpu,
            reason: Reason::WarmingUp,
        });
    }

    /// What makes the windows' renderers on the process's GPU.
    pub fn renderers(&self) -> GpuRenderers {
        self.renderers.clone()
    }

    /// Whether the window system draws on the GPU now.
    pub fn on(&self) -> bool {
        matches!(self.state, State::On(_))
    }

    /// Whether the device is still being made.
    pub fn warming(&self) -> bool {
        matches!(self.state, State::Warming)
    }

    /// The events for the runtime since the last call: the renderer changed.
    pub fn take_events(&mut self) -> Vec<PlatformEvent> {
        std::mem::take(&mut self.events)
    }

    /// When the run loop must look at the warm-up again even if nothing wakes it: the moment a warm-up still inside
    /// `Device::new` counts as hung ([`DEVICE_HANG`]).
    pub fn deadline(&self) -> Option<Instant> {
        if !self.warming() {
            return None;
        }
        match render_thread::status() {
            Status::Warming {
                since,
                device_made: false,
            } => Some(since + DEVICE_HANG),
            _ => None,
        }
    }

    /// Take the device once it is ready (without waiting); with `wait`, wait up to that long for it. A warm-up hung
    /// in `Device::new` is given up: the window system stays on the CPU, reported as a fallback.
    pub fn poll(&mut self, _conn: &mut Connection, wait: Option<Duration>) {
        if !self.warming() {
            return;
        }
        let status = match wait {
            None => render_thread::status(),
            Some(d) => render_thread::wait(d),
        };
        match status {
            Status::Warming { since, device_made } => {
                if !device_made && since.elapsed() >= DEVICE_HANG {
                    self.abandoned = self.claim.take().map(|(claim, _)| claim);
                    self.settle(Err(format!(
                        "the GPU device was not made within {} s",
                        DEVICE_HANG.as_secs()
                    )));
                }
            }
            // let go since it was asked for (a loss another window system saw): ask again
            Status::Idle => self.begin_warm(),
            Status::Failed(why) => self.settle(Err(why)),
            Status::Ready(ready) => self.take(&ready),
        }
    }

    /// Switch to the device `ready` names, or settle on the CPU when the rule says so.
    fn take(&mut self, ready: &Ready) {
        let facts = Ok((ready.info.software, ready.info.allocator.is_some()));
        let (kind, reason) =
            renderer::settle(self.requested, facts, self.losses.exceeded(Instant::now()));
        if kind != RendererKind::Gpu {
            self.state = State::Cpu;
            // the device goes unless another window system of the process draws on it
            self.claim = None;
            self.choose(Choice { kind, reason });
            return;
        }
        let sync = self.sync_node();
        let gpu = match GpuDevice::new(ready, &self.offered, sync) {
            Ok(gpu) => gpu,
            Err(cupertino_gpu::Error::Lost(loss)) => {
                // lost before this window system took it over
                render_thread::lost(ready.generation, loss);
                self.begin_warm_after(loss);
                return;
            }
            Err(e) => return self.settle(Err(e.to_string())),
        };
        let switched_after = self
            .claim
            .as_ref()
            .map_or(Duration::ZERO, |(_, asked)| asked.elapsed());
        let timing = ready.timing;
        eprintln!(
            "renderer: GPU ready {} ms after asking for it (context {} ms, programs {} ms, {} loaded and {} compiled), {} explicit sync",
            switched_after.as_millis(),
            timing.context.as_millis(),
            timing.programs.as_millis(),
            timing.cache.loaded,
            timing.cache.compiled,
            if gpu.explicit() { "with" } else { "without" },
        );
        self.warmed = Some((timing, switched_after));
        self.renderers.set(Some(gpu.generation()));
        self.state = State::On(Box::new(gpu));
        self.choose(Choice { kind, reason });
        self.events
            .push(PlatformEvent::RendererChanged(RendererKind::Gpu));
    }

    fn settle(&mut self, device: Result<(bool, bool), String>) {
        let (kind, reason) =
            renderer::settle(self.requested, device, self.losses.exceeded(Instant::now()));
        self.state = State::Cpu;
        self.claim = None;
        self.choose(Choice { kind, reason });
    }

    /// The render node's syncobj requests, when the compositor offers explicit sync and the kernel can wait on
    /// timeline points through an eventfd.
    fn sync_node(&self) -> Option<SyncNode> {
        if !self.explicit_offered {
            return None;
        }
        let node = SyncNode::open(self.node.as_deref()?).ok()?;
        node.supports_eventfd().then_some(node)
    }

    /// Whether `surface` presents from shared memory: the window system draws on the CPU (or is warming up), or the
    /// surface fell back to it.
    pub fn on_shm(&self, surface: SurfaceId) -> bool {
        match self.state {
            State::On(_) => matches!(self.surfaces.get(&surface), Some(SurfaceKind::Fallback)),
            _ => true,
        }
    }

    /// Present `frame` of `surface` on the GPU if the window system and the surface draw there.
    #[allow(clippy::too_many_arguments)]
    pub fn present(
        &mut self,
        conn: &mut Connection,
        surface: SurfaceId,
        preferred: (u32, u32),
        frame: &CpuFrame,
        damage: &[PixelRect],
        whole: bool,
    ) -> Result<Presented, cupertino_wayland_client::Error> {
        let State::On(gpu) = &self.state else {
            return Ok(Presented::Shm);
        };
        let kind = self
            .surfaces
            .entry(surface)
            .or_insert_with(|| SurfaceKind::Gpu(GpuSurface::default()));
        let SurfaceKind::Gpu(s) = kind else {
            return Ok(Presented::Shm);
        };
        match s.present(gpu, conn, surface, preferred, frame, damage, whole) {
            Ok(()) => Ok(Presented::Gpu),
            Err(PresentError::Busy) => Ok(Presented::Busy),
            Err(PresentError::Resized) => Ok(Presented::Resized),
            Err(e) => {
                self.failed(conn, surface, e)?;
                Ok(Presented::Shm)
            }
        }
    }

    /// Prepare a frame of `pixels` of `surface` to be drawn and committed on the render thread ([`Draw::draw`]) if the
    /// window system and the surface draw on the GPU; [`GpuPath::finish`] says how that went.
    pub fn prepare(
        &mut self,
        conn: &mut Connection,
        surface: SurfaceId,
        preferred: (u32, u32),
        pixels: (u32, u32),
        whole: bool,
    ) -> Result<Prepared, cupertino_wayland_client::Error> {
        let State::On(gpu) = &self.state else {
            return Ok(Prepared::Shm);
        };
        let kind = self
            .surfaces
            .entry(surface)
            .or_insert_with(|| SurfaceKind::Gpu(GpuSurface::default()));
        let SurfaceKind::Gpu(s) = kind else {
            return Ok(Prepared::Shm);
        };
        match s.prepare(gpu, conn, surface, preferred, pixels, whole) {
            Ok(draw) => {
                s.handed_over();
                Ok(Prepared::Gpu(Box::new(draw)))
            }
            Err(PresentError::Busy) => Ok(Prepared::Busy),
            Err(PresentError::Resized) => Ok(Prepared::Resized),
            Err(e) => {
                self.failed(conn, surface, e)?;
                Ok(Prepared::Shm)
            }
        }
    }

    /// A frame of `surface` [`GpuPath::prepare`] handed over was drawn (copied from its renderer's texture or not), or
    /// could not be: the surface's buffers go then, and a lost device or a surface falling back is handled as when
    /// presenting.
    pub fn finish(
        &mut self,
        conn: &mut Connection,
        surface: SurfaceId,
        drawn: Result<bool, PresentError>,
    ) -> Result<(), cupertino_wayland_client::Error> {
        let gpu = match &self.state {
            State::On(gpu) => Some(gpu.as_ref()),
            _ => None,
        };
        let Some(SurfaceKind::Gpu(s)) = self.surfaces.get_mut(&surface) else {
            return Ok(());
        };
        match drawn {
            Ok(from_texture) => {
                s.drawn(from_texture);
                Ok(())
            }
            Err(e) => {
                s.not_drawn(gpu, conn);
                self.failed(conn, surface, e)
            }
        }
    }

    /// Presenting on `surface` failed by `e`: it falls back to shared memory, or the device was lost.
    fn failed(
        &mut self,
        conn: &mut Connection,
        surface: SurfaceId,
        e: PresentError,
    ) -> Result<(), cupertino_wayland_client::Error> {
        match e {
            PresentError::Fallback(why) => {
                renderer::fallback(
                    FallbackScope::Surface,
                    &format!("the surface draws into shared memory: {why}"),
                );
                let gpu = match &self.state {
                    State::On(gpu) => Some(gpu.as_ref()),
                    _ => None,
                };
                if let Some(SurfaceKind::Gpu(s)) = self.surfaces.get_mut(&surface) {
                    s.free(gpu, conn);
                }
                self.surfaces.insert(surface, SurfaceKind::Fallback);
                Ok(())
            }
            PresentError::Lost(loss) => {
                self.lost(conn, loss);
                Ok(())
            }
            PresentError::Wayland(e) => Err(e),
            PresentError::Busy | PresentError::Resized => Ok(()),
        }
    }

    /// The device was reset or purged: every surface's GPU buffers go, the render thread lets the device go (once for
    /// the process), and the window system asks for a new one, or stays on the CPU after the third reset within a
    /// minute. Frames are drawn on the CPU meanwhile.
    pub fn lost(&mut self, conn: &mut Connection, loss: Loss) {
        let State::On(gpu) = std::mem::replace(&mut self.state, State::Cpu) else {
            return;
        };
        // renderers made from now on draw on the CPU; those on the lost device go with the switch back
        self.renderers.set(None);
        for (_, kind) in self.surfaces.drain() {
            if let SurfaceKind::Gpu(mut s) = kind {
                // the GL names went with the context; the buffers' images, buffer objects and timelines, which a loss
                // leaves, are freed with the protocol objects
                s.free(Some(&gpu), conn);
            }
        }
        render_thread::lost(gpu.generation(), loss);
        drop(gpu);
        let lost = format!("the GPU context was lost ({loss:?})");
        if loss.counts_as_failure() {
            renderer::fallback(FallbackScope::Loss, &lost);
        } else {
            // a purge (suspend, a VT switch, a mode set) is no fault: the next device takes over
            eprintln!("renderer: {lost}");
        }
        self.events
            .push(PlatformEvent::RendererChanged(RendererKind::Cpu));
        self.begin_warm_after(loss);
    }

    /// After `loss`: counted, then a new device asked for, or the CPU kept after the third reset within a minute.
    fn begin_warm_after(&mut self, loss: Loss) {
        if self.losses.record(loss.counts_as_failure(), Instant::now()) {
            self.claim = None;
            self.state = State::Cpu;
            self.choose(Choice {
                kind: RendererKind::Cpu,
                reason: Reason::Losses,
            });
        } else {
            self.begin_warm();
        }
    }

    /// The compositor says what it showed of the buffers is gone: check the device (a purge or a reset of it is
    /// handled as a loss) and upload every surface's next frame whole.
    pub fn buffers_lost(&mut self, conn: &mut Connection) {
        let status = match &self.state {
            State::On(gpu) => gpu.status(),
            _ => return,
        };
        if let Err(cupertino_gpu::Error::Lost(loss)) = status {
            self.lost(conn, loss);
            return;
        }
        for kind in self.surfaces.values_mut() {
            if let SurfaceKind::Gpu(s) = kind {
                s.forget();
            }
        }
    }

    /// Whether `surface` could present on the GPU now; `None` when it does not present there.
    pub fn buffer_free(&mut self, conn: &Connection, surface: SurfaceId) -> Option<bool> {
        let State::On(gpu) = &self.state else {
            return None;
        };
        match self.surfaces.get_mut(&surface) {
            Some(SurfaceKind::Gpu(s)) => Some(s.has_free_buffer(gpu, conn)),
            _ => None,
        }
    }

    /// Whether `surface` draws on the GPU (the window system does and the surface has not fallen back).
    pub fn draws(&self, surface: SurfaceId) -> bool {
        self.on() && !matches!(self.surfaces.get(&surface), Some(SurfaceKind::Fallback))
    }

    /// Whether `surface` fell back to shared memory.
    pub fn fell_back(&self, surface: SurfaceId) -> bool {
        matches!(self.surfaces.get(&surface), Some(SurfaceKind::Fallback))
    }

    /// What `surface`'s GPU presenting did.
    pub fn stats(&self, surface: SurfaceId) -> Option<SurfaceStats> {
        match self.surfaces.get(&surface) {
            Some(SurfaceKind::Gpu(s)) => Some(s.stats),
            _ => None,
        }
    }

    /// The surface is gone: free its buffers.
    pub fn forget(&mut self, conn: &mut Connection, surface: SurfaceId) {
        if let Some(waiting) = self.waiting.as_mut() {
            waiting.remove(&surface);
        }
        if let Some(SurfaceKind::Gpu(mut s)) = self.surfaces.remove(&surface) {
            let gpu = match &self.state {
                State::On(gpu) => Some(gpu.as_ref()),
                _ => None,
            };
            s.free(gpu, conn);
        }
    }

    /// Test hook: the device's next status check reads `loss` instead of the driver's own.
    pub fn inject_loss(&self, loss: Loss) {
        if let State::On(gpu) = &self.state {
            gpu.inject_loss(loss);
        }
    }

    /// Whether buffers are synchronised through syncobj timelines.
    pub fn explicit(&self) -> bool {
        matches!(&self.state, State::On(gpu) if gpu.explicit())
    }

    /// The device's renderer string, while the window system draws on the GPU.
    pub fn device_name(&self) -> Option<String> {
        match &self.state {
            State::On(gpu) => Some(gpu.info().renderer.clone()),
            _ => None,
        }
    }

    /// The modifiers buffers may use, while the window system draws on the GPU.
    pub fn modifiers(&self) -> Vec<u64> {
        match &self.state {
            State::On(gpu) => gpu.modifiers().to_vec(),
            _ => Vec::new(),
        }
    }
}

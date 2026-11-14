//! The GPU path of a process: deciding its renderer by the fallback rule ([`crate::renderer`]), making the device and
//! its programs on a worker thread while the first frames are drawn on the CPU, switching to the GPU once they are
//! ready (the windows' renderers then draw on the same device, [`GpuRenderers`]), the GPU buffers of each surface, falling back per surface to shared memory when a buffer cannot be made or
//! imported, and recovering from a lost context (three resets within a minute leave the process on the CPU; purges
//! never count).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use cupertino_gpu::syncobj::SyncNode;
use cupertino_gpu::{
    program_cache, DetachedDevice, Device, DeviceDesc, Loss, Platform, ProgramCacheStats,
    FOURCC_ARGB8888,
};
use cupertino_ui_gpu::{GpuConfig, GpuPrograms, GpuRenderer, WindowRenderer};
use cupertino_ui_platform::PlatformEvent;
use cupertino_ui_render::CpuFrame;
use cupertino_ui_text::GlyphRasterizer;
use cupertino_wayland_client::{Connection, PixelRect, SurfaceId};

use crate::gpu_present::{GpuDevice, GpuSurface, PresentError, Programs, SurfaceStats};
use crate::renderer::{
    self, Choice, LossHistory, Reason, RendererKind, Requested, Start, GPU_BY_DEFAULT,
};

/// What the warm-up thread made, and how long it took.
pub struct Warm {
    device: DetachedDevice,
    programs: Programs,
    pub timing: WarmTiming,
}

// Safety: the programs are GL names of the detached device's context, used only once the device is attached on the
// receiving thread; they carry no thread-bound state of their own.
unsafe impl Send for Warm {}

/// How long the warm-up took.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WarmTiming {
    /// Making the context on the device.
    pub context: Duration,
    /// Making the programs (compiled, or loaded from saved binaries).
    pub programs: Duration,
    pub cache: ProgramCacheStats,
}

/// Make the device on `node` (or the driver's default device), with saved programs under `cache`, and its programs.
pub fn warm(node: Option<&Path>, cache: Option<&Path>) -> Result<Warm, String> {
    let started = Instant::now();
    let platform = match node {
        Some(node) => Platform::RenderNode(node.to_path_buf()),
        None => Platform::Surfaceless,
    };
    let mut device = Device::new(&DeviceDesc { platform }).map_err(|e| e.to_string())?;
    let context = started.elapsed();
    device.set_program_cache(cache);
    let compiling = Instant::now();
    let programs = Programs::new(&device).map_err(|e| e.to_string())?;
    let timing = WarmTiming {
        context,
        programs: compiling.elapsed(),
        cache: device.program_cache_stats(),
    };
    let device = device.detach().map_err(|e| e.to_string())?;
    Ok(Warm {
        device,
        programs,
        timing,
    })
}

/// The render node of the device `dev` names (a `dev_t` from dmabuf feedback): the node itself when it is one, else
/// the render node of the same device.
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
}

/// The major number of a Linux `dev_t`.
fn libc_major(dev: u64) -> u64 {
    ((dev >> 32) & 0xffff_f000) | ((dev >> 8) & 0x0000_0fff)
}

/// The minor number of a Linux `dev_t`.
fn libc_minor(dev: u64) -> u64 {
    ((dev >> 12) & 0xffff_ff00) | (dev & 0x0000_00ff)
}

/// What makes the windows' renderers on the process's GPU: its device and the renderers' programs while the process
/// draws on the GPU, nothing otherwise. A renderer factory holds a clone and asks it for a renderer each time it makes
/// one, so the renderers made after a switch ([`PlatformEvent::RendererChanged`]) draw on the device the frames are
/// presented from.
#[derive(Clone, Default)]
pub struct GpuRenderers(Rc<RefCell<Option<OnDevice>>>);

/// The process's device and the renderers' programs on it.
type OnDevice = (Rc<Device>, Rc<GpuPrograms>);

impl GpuRenderers {
    /// A window's renderer on the process's GPU, or `None` while the process draws on the CPU.
    pub fn renderer<R: GlyphRasterizer>(
        &self,
        config: GpuConfig,
        rasterizer: R,
    ) -> Option<WindowRenderer<R>> {
        let (device, programs) = self.0.borrow().clone()?;
        device.status().ok()?;
        Some(WindowRenderer::new(GpuRenderer::with_programs(
            device, programs, config, rasterizer,
        )))
    }

    /// Whether renderers are made on the GPU now.
    pub fn on(&self) -> bool {
        self.0.borrow().is_some()
    }

    fn set(&self, on: Option<OnDevice>) {
        *self.0.borrow_mut() = on;
    }
}

enum State {
    /// The process draws on the CPU.
    Cpu,
    /// The device and programs are being made; frames are drawn on the CPU meanwhile.
    Warming {
        rx: Receiver<Result<Warm, String>>,
        started: Instant,
    },
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
    /// Not for the GPU (the process or the surface draws into shared memory): present it there.
    Shm,
    /// Every buffer is the compositor's still.
    Busy,
    /// The frame is not of the size the surface asks for now.
    Resized,
}

/// The process's GPU path.
pub struct GpuPath {
    state: State,
    requested: Requested,
    node: Option<PathBuf>,
    offered: Vec<u64>,
    explicit_offered: bool,
    losses: LossHistory,
    surfaces: HashMap<SurfaceId, SurfaceKind>,
    cache: Option<PathBuf>,
    /// The warm-up's timing and when the switch happened after the start, once the GPU took over.
    pub warmed: Option<(WarmTiming, Duration)>,
    /// Events for the runtime: a change of renderer.
    events: Vec<PlatformEvent>,
    renderers: GpuRenderers,
}

impl GpuPath {
    /// Decide by the rule (asked through `CUPERTINO_RENDERER`) and start the warm-up when the GPU may be used.
    pub fn start(conn: &mut Connection) -> GpuPath {
        Self::start_with(conn, Requested::from_env(), GPU_BY_DEFAULT)
    }

    pub fn start_with(conn: &mut Connection, requested: Requested, turned_on: bool) -> GpuPath {
        let mut path = GpuPath {
            state: State::Cpu,
            requested,
            node: None,
            offered: Vec::new(),
            explicit_offered: false,
            losses: LossHistory::default(),
            surfaces: HashMap::new(),
            cache: program_cache::default_dir(),
            warmed: None,
            events: Vec::new(),
            renderers: GpuRenderers::default(),
        };
        let wants_device = match requested {
            Requested::Cpu => false,
            Requested::Default => turned_on,
            Requested::Gpu => true,
        };
        // a process that draws on the CPU never binds the dmabuf protocol
        let feedback = if wants_device {
            conn.dmabuf_feedback().ok().flatten()
        } else {
            None
        };
        path.offered = feedback
            .as_ref()
            .map(|f| f.modifiers(FOURCC_ARGB8888))
            .unwrap_or_default();
        path.explicit_offered = wants_device && conn.has_explicit_sync().unwrap_or(false);
        let node = feedback.map(|f| f.main_device.and_then(render_node_of));
        match renderer::start(requested, turned_on, node) {
            Start::Cpu(reason) => renderer::set_active(Choice {
                kind: RendererKind::Cpu,
                reason,
            }),
            Start::Device { node } => {
                path.node = node;
                path.begin_warm(conn);
            }
        }
        path
    }

    fn begin_warm(&mut self, conn: &Connection) {
        let (tx, rx) = mpsc::channel();
        let (node, cache) = (self.node.clone(), self.cache.clone());
        let waker = conn.waker();
        let spawned = std::thread::Builder::new()
            .name("gpu-warm-up".into())
            .spawn(move || {
                let _ = tx.send(warm(node.as_deref(), cache.as_deref()));
                waker.wake();
            });
        match spawned {
            Ok(_) => {
                self.state = State::Warming {
                    rx,
                    started: Instant::now(),
                };
                renderer::set_active(Choice {
                    kind: RendererKind::Cpu,
                    reason: Reason::WarmingUp,
                });
            }
            Err(e) => self.settle(Err(format!("no warm-up thread: {e}"))),
        }
    }

    /// What makes the windows' renderers on the process's GPU.
    pub fn renderers(&self) -> GpuRenderers {
        self.renderers.clone()
    }

    /// Whether the process draws on the GPU now.
    pub fn on(&self) -> bool {
        matches!(self.state, State::On(_))
    }

    /// Whether the device is still being made.
    pub fn warming(&self) -> bool {
        matches!(self.state, State::Warming { .. })
    }

    /// The events for the runtime since the last call: the renderer changed.
    pub fn take_events(&mut self) -> Vec<PlatformEvent> {
        std::mem::take(&mut self.events)
    }

    /// Take the warm-up's result when it is ready (without waiting); with `wait`, wait up to that long for it.
    pub fn poll(&mut self, conn: &mut Connection, wait: Option<Duration>) {
        let State::Warming { rx, started } = &self.state else {
            return;
        };
        let started = *started;
        let result = match wait {
            None => match rx.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Err("the warm-up thread ended".into()),
            },
            Some(d) => match rx.recv_timeout(d) {
                Ok(r) => r,
                Err(mpsc::RecvTimeoutError::Timeout) => return,
                Err(mpsc::RecvTimeoutError::Disconnected) => Err("the warm-up thread ended".into()),
            },
        };
        let switched_after = started.elapsed();
        match result.and_then(|w| {
            let timing = w.timing;
            w.device
                .attach()
                .map(|d| (d, w.programs, timing))
                .map_err(|e| e.to_string())
        }) {
            Ok((device, programs, timing)) => {
                let facts = Ok((device.info().software, device.info().allocator.is_some()));
                let (kind, reason) =
                    renderer::settle(self.requested, facts, self.losses.exceeded(Instant::now()));
                if kind == RendererKind::Gpu {
                    let sync = self.sync_node();
                    let gpu = GpuDevice::new(device, programs, &self.offered, sync);
                    eprintln!(
                        "renderer: GPU ready {} ms after the start (context {} ms, programs {} ms, {} loaded and {} compiled), {} explicit sync",
                        switched_after.as_millis(),
                        timing.context.as_millis(),
                        timing.programs.as_millis(),
                        timing.cache.loaded,
                        timing.cache.compiled,
                        if gpu.explicit() { "with" } else { "without" },
                    );
                    self.warmed = Some((timing, switched_after));
                    self.renderers
                        .set(Some((Rc::clone(&gpu.device), Rc::clone(&gpu.ui))));
                    self.state = State::On(Box::new(gpu));
                    renderer::set_active(Choice { kind, reason });
                    self.events
                        .push(PlatformEvent::RendererChanged(RendererKind::Gpu));
                    let _ = conn;
                } else {
                    self.state = State::Cpu;
                    renderer::set_active(Choice { kind, reason });
                }
            }
            Err(why) => self.settle(Err(why)),
        }
    }

    fn settle(&mut self, device: Result<(bool, bool), String>) {
        let (kind, reason) =
            renderer::settle(self.requested, device, self.losses.exceeded(Instant::now()));
        self.state = State::Cpu;
        renderer::set_active(Choice { kind, reason });
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

    /// Present `frame` of `surface` on the GPU if the process and the surface draw there.
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
            Err(PresentError::Fallback(why)) => {
                eprintln!("renderer: surface draws into shared memory: {why}");
                s.free(Some(gpu), conn);
                self.surfaces.insert(surface, SurfaceKind::Fallback);
                Ok(Presented::Shm)
            }
            Err(PresentError::Lost(loss)) => {
                self.lost(conn, loss);
                Ok(Presented::Shm)
            }
            Err(PresentError::Wayland(e)) => Err(e),
        }
    }

    /// The device was reset or purged: every surface's GPU buffers go, and the process makes a new device, or stays on
    /// the CPU after the third reset within a minute. Frames are drawn on the CPU meanwhile.
    pub fn lost(&mut self, conn: &mut Connection, loss: Loss) {
        let State::On(gpu) = std::mem::replace(&mut self.state, State::Cpu) else {
            return;
        };
        // renderers made from now on draw on the CPU; those on the lost device go with the switch back
        self.renderers.set(None);
        for (_, kind) in self.surfaces.drain() {
            if let SurfaceKind::Gpu(mut s) = kind {
                // the textures went with the context; the protocol objects are destroyed
                s.free(None, conn);
            }
        }
        drop(gpu);
        eprintln!("renderer: the GPU context was lost ({loss:?})");
        let exceeded = self.losses.record(loss.counts_as_failure(), Instant::now());
        self.events
            .push(PlatformEvent::RendererChanged(RendererKind::Cpu));
        if exceeded {
            renderer::set_active(Choice {
                kind: RendererKind::Cpu,
                reason: Reason::Losses,
            });
        } else {
            self.begin_warm(conn);
        }
    }

    /// The compositor says what it showed of the buffers is gone: check the own device (a purge or a reset of it is
    /// handled as a loss) and upload every surface's next frame whole.
    pub fn buffers_lost(&mut self, conn: &mut Connection) {
        let status = match &self.state {
            State::On(gpu) => gpu.device.status(),
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

    /// Whether `surface` draws on the GPU (the process does and the surface has not fallen back).
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
            gpu.device.inject_loss(loss);
        }
    }

    /// Whether buffers are synchronised through syncobj timelines.
    pub fn explicit(&self) -> bool {
        matches!(&self.state, State::On(gpu) if gpu.explicit())
    }

    /// The device's renderer string, while the process draws on the GPU.
    pub fn device_name(&self) -> Option<String> {
        match &self.state {
            State::On(gpu) => Some(gpu.device.info().renderer.clone()),
            _ => None,
        }
    }

    /// The modifiers buffers may use, while the process draws on the GPU.
    pub fn modifiers(&self) -> Vec<u64> {
        match &self.state {
            State::On(gpu) => gpu.modifiers().to_vec(),
            _ => Vec::new(),
        }
    }
}

impl Drop for GpuPath {
    /// A process that ends while its device is still being made waits for the warm-up thread (a little while at most):
    /// the driver must not be torn down under a thread still compiling in it.
    fn drop(&mut self) {
        if let State::Warming { rx, .. } = &self.state {
            if let Ok(Ok(warm)) = rx.recv_timeout(WARM_UP_WAIT_AT_EXIT) {
                // the context is made current here to be destroyed
                drop(warm.device.attach());
            }
        }
    }
}

/// The longest a process ending during its warm-up waits for the warm-up thread.
const WARM_UP_WAIT_AT_EXIT: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dev_t_splits_into_the_kernel_s_major_and_minor() {
        // makedev(226, 128) and makedev(226, 1), as the kernel encodes them
        let render = (226u64 << 8) | 128;
        assert_eq!((libc_major(render), libc_minor(render)), (226, 128));
        let big = ((0x1234u64 & 0xfffff000) << 32)
            | ((0x1234u64 & 0xfff) << 8)
            | ((0x56789u64 & 0xffffff00) << 12)
            | (0x56789u64 & 0xff);
        assert_eq!((libc_major(big), libc_minor(big)), (0x1234, 0x56789));
    }
}

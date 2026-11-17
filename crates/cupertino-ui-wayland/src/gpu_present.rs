//! Presenting a surface's frames from dmabufs drawn on the GPU.
//!
//! A [`GpuSurface`] keeps the buffers of one surface: two with explicit sync (each with a syncobj timeline: the
//! frame's native fence becomes its acquire point, and the buffer is drawn into again once its release point
//! signalled), three without (the client waits for its own fence before it commits, and draws into a buffer again only
//! after the compositor released it and one further frame went out, or, with none such, into the one it released
//! first: [`pick`]). Buffers are allocated in size steps
//! ([`allocation`]) and cropped to the frame by the surface's viewport, so a live resize allocates once a step, not
//! once a frame. A buffer uses only a modifier the compositor offers and the driver renders to; linear is never added
//! on a hardware driver ([`usable_modifiers`]).
//!
//! The drawing itself happens on the process's render thread ([`render_thread`]), which holds the buffers' textures;
//! the window system's thread keeps the buffers' protocol objects and sync points and commits. A frame a window's
//! renderer drew there ([`cupertino_ui_render::gpu::WindowRenderer`]) is copied from its texture into the buffer, texel
//! for texel from the first pixel (the buffer may be larger by the size step), and nothing is uploaded. A frame of the
//! CPU renderer has its pixels uploaded into a texture of the buffer's size, only what the frame changed, and drawn
//! over the buffer; a frame drawn on another device is read back first.

use std::os::fd::{AsFd, OwnedFd};
use std::rc::Rc;
use std::time::Duration;

use cupertino_gpu::syncobj::{self, SyncNode, Timeline};
use cupertino_gpu::{
    shaders, AddressMode, BindGroupEntry, BindGroupLayout, BindingKind, BindingLayout,
    BindingResource, Blend, Device, DeviceInfo, DmabufDesc, Error as GpuError, Filter, LoadOp,
    Loss, Pipeline, PipelineDesc, Region, Sampler, SamplerDesc, Texture, TextureDesc,
    TextureFormat, TextureUsage, Topology, FOURCC_ARGB8888, MODIFIER_INVALID, MODIFIER_LINEAR,
};
use cupertino_ui_render::gpu::WindowFrame;
use cupertino_ui_render::render_thread::{self, Ready, Resident};
use cupertino_ui_render::CpuFrame;
use cupertino_wayland_client::{
    Connection, DmabufAttributes, DmabufPlaneRef, Error as WlError, GpuBufferId, GpuFrame,
    PixelRect, SurfaceId, TimelineId,
};

/// The step buffers grow by while a window resizes, in pixels on each axis: a buffer is reused while the frame fits
/// it and is less than two steps smaller.
pub const SIZE_STEP: u32 = 128;

/// The longest the client waits for its own fence before a commit without explicit sync.
const FENCE_WAIT: Duration = Duration::from_secs(2);

/// The buffers a surface keeps: two when release points tell when each is free, three otherwise.
pub fn buffer_count(explicit: bool) -> usize {
    if explicit {
        2
    } else {
        3
    }
}

/// The size to allocate for a frame of `frame` pixels: rounded up to [`SIZE_STEP`] when the surface can crop (and
/// within `max`), the frame's own size otherwise.
pub fn allocation(frame: (u32, u32), can_crop: bool, max: u32) -> (u32, u32) {
    if !can_crop {
        return frame;
    }
    let up = |v: u32| {
        v.div_ceil(SIZE_STEP)
            .saturating_mul(SIZE_STEP)
            .min(max)
            .max(v)
    };
    (up(frame.0), up(frame.1))
}

/// Whether buffers of `allocated` pixels serve a frame of `frame`: it fits, and on a surface that crops it is less
/// than two steps smaller (a surface that cannot crop needs the frame's size exactly).
pub fn serves(allocated: (u32, u32), frame: (u32, u32), can_crop: bool) -> bool {
    if !can_crop {
        return allocated == frame;
    }
    let axis = |a: u32, f: u32| f <= a && a - f < 2 * SIZE_STEP;
    axis(allocated.0, frame.0) && axis(allocated.1, frame.1)
}

/// The modifiers a buffer may use: those the compositor offers for the format that the driver renders to, in the
/// compositor's order. Linear is kept only when both list it, so a hardware driver never gets linear unless it renders
/// to it and the compositor asked for it; the implicit layout is kept when the compositor offers it.
pub fn usable_modifiers(offered: &[u64], renderable: &[u64], software: bool) -> Vec<u64> {
    offered
        .iter()
        .copied()
        .filter(|m| renderable.contains(m) || *m == MODIFIER_INVALID)
        .filter(|&m| software || m != MODIFIER_LINEAR || renderable.contains(&MODIFIER_LINEAR))
        .collect()
}

/// When a buffer may be drawn into again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reuse {
    /// Never shown, or free again.
    Free,
    /// Waiting for its release point (explicit sync).
    ReleasePoint,
    /// Without explicit sync: waiting for the compositor's release.
    Release,
    /// Without explicit sync: released when commit number `.0` was the latest; free once a later commit went out.
    ReleasedAt(u64),
}

impl Reuse {
    /// Whether the buffer is free when `commits` frames were committed in all.
    pub fn free(self, commits: u64) -> bool {
        match self {
            Reuse::Free => true,
            Reuse::ReleasePoint | Reuse::Release => false,
            Reuse::ReleasedAt(at) => commits > at,
        }
    }
}

/// The buffer to draw the next frame into, of buffers in states `reuses` when `commits` frames were committed in all:
/// the first free one, else the one released longest ago. Waiting for a later commit gains nothing once every buffer
/// is either released or held: only a commit raises the count, and a buffer the compositor releases meanwhile is
/// stamped with the same count, so two frames committed back to back (as a configure lets a window do) would otherwise
/// leave none free for good while the third is shown. `None` while the compositor holds them all.
pub fn pick(reuses: impl IntoIterator<Item = Reuse>, commits: u64) -> Option<usize> {
    let mut oldest: Option<(u64, usize)> = None;
    for (i, reuse) in reuses.into_iter().enumerate() {
        if reuse.free(commits) {
            return Some(i);
        }
        if let Reuse::ReleasedAt(at) = reuse {
            if oldest.is_none_or(|(o, _)| at < o) {
                oldest = Some((at, i));
            }
        }
    }
    oldest.map(|(_, i)| i)
}

/// What a surface's next frame draws into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// The buffer at this index.
    Buffer(usize),
    /// A buffer allocated for it: the surface has fewer than it keeps.
    Allocate,
    /// None: the compositor holds every buffer.
    Busy,
}

/// The buffer a surface draws its next frame into, of buffers in states `reuses` when `commits` frames were committed
/// in all, keeping `count` ([`buffer_count`]): a free one, else a new one while there are fewer than `count`, else
/// [`pick`]'s.
pub fn next_buffer(reuses: &[Reuse], commits: u64, count: usize) -> Next {
    if let Some(i) = reuses.iter().position(|r| r.free(commits)) {
        return Next::Buffer(i);
    }
    if reuses.len() < count {
        return Next::Allocate;
    }
    pick(reuses.iter().copied(), commits).map_or(Next::Busy, Next::Buffer)
}

/// A buffer's state after a look at the compositor's releases without explicit sync, `busy` being whether the
/// compositor still holds it, `commits` frames having been committed.
pub fn after_release(reuse: Reuse, busy: bool, commits: u64) -> Reuse {
    match reuse {
        Reuse::Release if !busy => Reuse::ReleasedAt(commits),
        other => other,
    }
}

/// Why a frame was not shown from a GPU buffer.
#[derive(Debug)]
pub enum PresentError {
    /// Every buffer is still the compositor's: present again once one is free.
    Busy,
    /// The frame is not of the size the surface asks for now; one of that size replaces it.
    Resized,
    /// The device was reset or its memory purged.
    Lost(Loss),
    /// Allocating or importing a buffer failed: the surface draws into shared memory from now on.
    Fallback(String),
    Wayland(WlError),
}

impl From<WlError> for PresentError {
    fn from(e: WlError) -> Self {
        PresentError::Wayland(e)
    }
}

fn gpu_error(e: GpuError) -> PresentError {
    match e {
        GpuError::Lost(loss) => PresentError::Lost(loss),
        other => PresentError::Fallback(other.to_string()),
    }
}

/// The process's GPU device as a window system presents with it: the device itself is the render thread's
/// ([`render_thread`]), shared with the windows' renderers; this keeps which one it is, what the window system learnt
/// of it, and the programs its surfaces draw with there.
pub struct GpuDevice {
    generation: u64,
    info: DeviceInfo,
    /// The render node's syncobj requests, when explicit sync is used.
    sync: Option<SyncNode>,
    /// The modifiers buffers may use.
    modifiers: Vec<u64>,
    programs: Resident<Programs>,
}

/// The programs a surface draws with, on the render thread; freed with the last of them.
pub struct Programs {
    device: Rc<Device>,
    blit: Option<Pipeline>,
    layout: BindGroupLayout,
    sampler: Option<Sampler>,
    /// A renderer's texture copied into a buffer texel for texel.
    texels: Option<Pipeline>,
    texels_layout: BindGroupLayout,
}

impl Programs {
    pub fn new(device: &Rc<Device>) -> Result<Programs, GpuError> {
        let layout = device.create_bind_group_layout(&[
            BindingLayout {
                binding: 1,
                kind: BindingKind::Texture,
            },
            BindingLayout {
                binding: 2,
                kind: BindingKind::Sampler,
            },
        ])?;
        let blit = device.create_pipeline(&PipelineDesc {
            shader: &shaders::BLIT,
            layouts: &[&layout],
            vertex_buffers: &[],
            blend: Blend::Replace,
            topology: Topology::TriangleList,
        })?;
        let sampler = device.create_sampler(&SamplerDesc {
            filter: Filter::Nearest,
            address: AddressMode::ClampToEdge,
        })?;
        let texels_layout = device.create_bind_group_layout(&[BindingLayout {
            binding: 1,
            kind: BindingKind::Texture,
        }])?;
        let texels = device.create_pipeline(&PipelineDesc {
            shader: &shaders::BLIT_TEXELS,
            layouts: &[&texels_layout],
            vertex_buffers: &[],
            blend: Blend::Replace,
            topology: Topology::TriangleList,
        })?;
        Ok(Programs {
            device: Rc::clone(device),
            blit: Some(blit),
            layout,
            sampler: Some(sampler),
            texels: Some(texels),
            texels_layout,
        })
    }

    fn blit(&self) -> &Pipeline {
        self.blit.as_ref().expect("held until dropped")
    }

    fn texels(&self) -> &Pipeline {
        self.texels.as_ref().expect("held until dropped")
    }

    fn sampler(&self) -> &Sampler {
        self.sampler.as_ref().expect("held until dropped")
    }
}

impl Drop for Programs {
    fn drop(&mut self) {
        for pipeline in self.blit.take().into_iter().chain(self.texels.take()) {
            self.device.destroy_pipeline(pipeline);
        }
        if let Some(sampler) = self.sampler.take() {
            self.device.destroy_sampler(sampler);
        }
    }
}

impl GpuDevice {
    /// The process's device `ready` names, presenting buffers in modifiers of `offered` (ARGB8888's in the
    /// compositor's feedback), with explicit sync through `sync` when given; its surfaces' programs are made now, on
    /// the render thread.
    pub fn new(
        ready: &Ready,
        offered: &[u64],
        sync: Option<SyncNode>,
    ) -> Result<GpuDevice, GpuError> {
        let generation = ready.generation;
        let programs = render_thread::run(move |on| {
            let programs = Programs::new(on.device_of(generation)?)?;
            Ok::<_, GpuError>(Resident::new(on, programs))
        })?;
        let info = ready.info.clone();
        let modifiers = match info.allocator {
            // the export allocator takes the driver's layout, which the offer must hold
            Some(cupertino_gpu::Allocator::Export) => offered.to_vec(),
            _ => usable_modifiers(offered, &ready.renderable, info.software),
        };
        let sync = sync.filter(|_| info.native_fences);
        Ok(GpuDevice {
            generation,
            info,
            sync,
            modifiers,
            programs,
        })
    }

    /// Which of the process's devices it is ([`Ready::generation`]).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    /// Whether buffers are synchronised through syncobj timelines.
    pub fn explicit(&self) -> bool {
        self.sync.is_some()
    }

    pub fn modifiers(&self) -> &[u64] {
        &self.modifiers
    }

    /// Whether the device still works: `Lost` once it was reset or purged, or let go for a loss another window system
    /// of the process saw.
    pub fn status(&self) -> Result<(), GpuError> {
        let generation = self.generation;
        render_thread::run(move |on| on.device_of(generation).map(|_| ()))
    }

    /// Test hook: the device's next status check reads `loss` instead of the driver's own.
    pub fn inject_loss(&self, loss: Loss) {
        let generation = self.generation;
        render_thread::run(move |on| {
            if on.generation() == Some(generation) {
                if let Some(device) = on.device() {
                    device.inject_loss(loss);
                }
            }
        });
    }
}

struct Slot {
    buffer: GpuBufferId,
    timeline: Option<(Timeline, TimelineId)>,
    uses: u64,
    reuse: Reuse,
    release: Option<OwnedFd>,
}

/// What a surface's GPU presenting did, for tests and reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SurfaceStats {
    pub frames: u64,
    /// Buffers allocated, in all (a resize across a step allocates the surface's buffers again).
    pub allocations: u64,
    /// Buffers made free by their release point.
    pub release_waits: u64,
    /// Own fences waited for before a commit (without explicit sync).
    pub fence_waits: u64,
    /// Frames copied from a renderer's texture on the device, with nothing uploaded.
    pub from_texture: u64,
    /// The size buffers have now.
    pub allocated: (u32, u32),
    pub modifier: u64,
}

/// The textures of a surface's buffers, one per slot, and its staging texture, on the render thread; freed with them
/// on the device they were made on, which a loss leaves their images and buffer objects to.
struct Images {
    device: Rc<Device>,
    buffers: Vec<Texture>,
    staging: Option<Texture>,
}

impl Images {
    /// Free the texture of the buffer allocated last.
    fn free_last(&mut self) {
        if let Some(texture) = self.buffers.pop() {
            self.device.destroy_texture(texture);
        }
    }
}

impl Drop for Images {
    fn drop(&mut self) {
        for texture in self.buffers.drain(..).chain(self.staging.take()) {
            self.device.destroy_texture(texture);
        }
    }
}

/// How a frame drawn into a buffer is finished before its commit.
#[derive(Clone, Copy)]
enum Submit {
    /// With the frame's native fence.
    Fence,
    /// Waiting for the GPU to finish it: a driver without native fences.
    Finish,
}

/// The GPU buffers of one surface.
#[derive(Default)]
pub struct GpuSurface {
    slots: Vec<Slot>,
    images: Option<Resident<Images>>,
    allocated: Option<(u32, u32)>,
    commits: u64,
    /// The staging texture no longer holds the last frame: the next upload is whole.
    stale: bool,
    pub stats: SurfaceStats,
}

impl GpuSurface {
    /// Refresh which buffers are free: a signalled release point, or without explicit sync a release and one more
    /// commit.
    fn refresh(&mut self, gpu: &GpuDevice, conn: &Connection) {
        let commits = self.commits;
        for slot in &mut self.slots {
            match slot.reuse {
                Reuse::ReleasePoint => {
                    let signalled = slot
                        .release
                        .as_ref()
                        .is_some_and(|fd| syncobj::wait_readable(fd.as_fd(), 0))
                        || matches!((&gpu.sync, &slot.timeline), (Some(sync), Some((t, _))) if sync.signalled(t, 2 * slot.uses));
                    if signalled {
                        slot.release = None;
                        slot.reuse = Reuse::Free;
                        self.stats.release_waits += 1;
                    }
                }
                Reuse::Release => {
                    slot.reuse =
                        after_release(slot.reuse, conn.gpu_buffer_busy(slot.buffer), commits);
                }
                _ => {}
            }
        }
    }

    /// Whether a frame could be drawn now: a buffer is free or another may be allocated.
    pub fn has_free_buffer(&mut self, gpu: &GpuDevice, conn: &Connection) -> bool {
        self.refresh(gpu, conn);
        self.next(gpu) != Next::Busy
    }

    /// What the next frame draws into, as the buffers' states stand.
    fn next(&self, gpu: &GpuDevice) -> Next {
        let reuses: Vec<Reuse> = self.slots.iter().map(|s| s.reuse).collect();
        next_buffer(&reuses, self.commits, buffer_count(gpu.explicit()))
    }

    /// The staging texture no longer holds the last frame (after a purge): the next present uploads all of it.
    pub fn forget(&mut self) {
        self.stale = true;
    }

    /// Free every buffer, texture and timeline; the textures go on the render thread.
    pub fn free(&mut self, gpu: Option<&GpuDevice>, conn: &mut Connection) {
        for slot in self.slots.drain(..) {
            conn.destroy_gpu_buffer(slot.buffer);
            if let Some((timeline, id)) = slot.timeline {
                conn.destroy_timeline(id);
                if let Some(sync) = gpu.and_then(|g| g.sync.as_ref()) {
                    sync.destroy(timeline);
                }
            }
        }
        self.images = None;
        self.allocated = None;
    }

    fn allocate(
        &mut self,
        gpu: &GpuDevice,
        conn: &mut Connection,
        size: (u32, u32),
    ) -> Result<(), PresentError> {
        let (generation, modifiers) = (gpu.generation, gpu.modifiers.clone());
        let images = &mut self.images;
        let dmabuf = render_thread::run(move |on| {
            let device = on.device_of(generation)?;
            let image = device.allocate_dmabuf(&DmabufDesc {
                size,
                fourcc: FOURCC_ARGB8888,
                modifiers,
            })?;
            let images = images.get_or_insert_with(|| {
                Resident::new(
                    on,
                    Images {
                        device: Rc::clone(device),
                        buffers: Vec::new(),
                        staging: None,
                    },
                )
            });
            images.get_mut(on).buffers.push(image.texture);
            Ok(image.dmabuf)
        })
        .map_err(gpu_error)?;
        let planes = dmabuf
            .planes
            .iter()
            .map(|p| DmabufPlaneRef {
                fd: p.fd.as_fd(),
                offset: p.offset,
                stride: p.stride,
            })
            .collect();
        let made = conn.create_dmabuf_buffer(&DmabufAttributes {
            size,
            fourcc: dmabuf.fourcc,
            modifier: dmabuf.modifier,
            planes,
        });
        let buffer = match made {
            Ok(b) => b,
            Err(e) => {
                self.free_last();
                return Err(match e {
                    WlError::BufferRejected | WlError::Unsupported(_) => {
                        PresentError::Fallback(e.to_string())
                    }
                    other => PresentError::Wayland(other),
                });
            }
        };
        let timeline = match &gpu.sync {
            Some(sync) => {
                let made = sync
                    .timeline()
                    .and_then(|t| sync.export(&t).map(|fd| (t, fd)));
                match made {
                    Ok((t, fd)) => Some((t, conn.import_timeline(fd.as_fd())?)),
                    Err(e) => {
                        conn.destroy_gpu_buffer(buffer);
                        self.free_last();
                        return Err(PresentError::Fallback(format!("syncobj: {e}")));
                    }
                }
            }
            None => None,
        };
        self.stats.allocations += 1;
        self.stats.modifier = dmabuf.modifier;
        self.slots.push(Slot {
            buffer,
            timeline,
            uses: 0,
            reuse: Reuse::Free,
            release: None,
        });
        Ok(())
    }

    /// Free the texture of the buffer allocated last, whose `wl_buffer` could not be made.
    fn free_last(&mut self) {
        if let Some(images) = &mut self.images {
            render_thread::run(|on| images.get_mut(on).free_last());
        }
    }

    /// Show `frame` on `surface`: copied from its renderer's texture on this device, or its `damage` (pixels;
    /// everything when `whole`) uploaded, drawn into a free buffer on the render thread, synchronised and committed.
    #[allow(clippy::too_many_arguments)]
    pub fn present(
        &mut self,
        gpu: &GpuDevice,
        conn: &mut Connection,
        surface: SurfaceId,
        preferred: (u32, u32),
        frame: &CpuFrame,
        damage: &[PixelRect],
        whole: bool,
    ) -> Result<(), PresentError> {
        let size = conn.frame_size(surface, preferred)?;
        let pixels = (frame.width, frame.height);
        if (size.width, size.height) != pixels {
            return Err(PresentError::Resized);
        }
        // a frame a window's renderer drew on the render thread is copied from its texture there; any other has its
        // pixels uploaded, read back first when they stayed on another device
        let source = frame
            .device
            .as_ref()
            .and_then(|d| d.0.as_any().downcast_ref::<WindowFrame>())
            .filter(|w| w.size() == pixels);
        let read_back;
        let frame = if source.is_none() && frame.pixels.is_empty() {
            read_back = frame.in_memory().map_err(|_| PresentError::Resized)?;
            &*read_back
        } else {
            frame
        };
        let can_crop = conn.can_crop(surface)?;
        // the compositor's damage is the frame's unless the frame is whole or the buffers are new
        let mut shown_whole = whole;
        let mut whole = whole || self.stale;
        if !self.allocated.is_some_and(|a| serves(a, pixels, can_crop)) {
            self.free(Some(gpu), conn);
            let max = gpu.info.max_texture_size;
            self.allocated = Some(allocation(pixels, can_crop, max));
            whole = true;
            shown_whole = true;
        }
        let allocated = self.allocated.expect("set above");
        self.stats.allocated = allocated;
        let leasing = tracing::trace_span!("wl.lease", gpu = true).entered();
        self.refresh(gpu, conn);
        let index = match self.next(gpu) {
            Next::Buffer(i) => i,
            Next::Allocate => {
                let _allocate = tracing::trace_span!("wl.allocate").entered();
                self.allocate(gpu, conn, allocated)?;
                self.slots.len() - 1
            }
            Next::Busy => return Err(PresentError::Busy),
        };
        drop(leasing);
        let submit = if gpu.sync.is_some() || gpu.info.native_fences {
            Submit::Fence
        } else {
            Submit::Finish
        };
        let generation = gpu.generation;
        let programs = &gpu.programs;
        let images = self
            .images
            .as_mut()
            .expect("a surface with buffers has their images");
        let (width, height, stride) = (frame.width, frame.height, frame.stride as usize);
        let frame_pixels: &[u8] = &frame.pixels;
        let drawn = render_thread::run(|on| {
            let device = on.device_of(generation).map_err(gpu_error)?;
            let programs = programs.get(on);
            let images = images.get_mut(on);
            match source {
                Some(source) => {
                    let texture = source.texture(on);
                    // a frame of a device given up (a loss) is not shown: the next comes from a renderer of this one
                    if !Rc::ptr_eq(texture.device(), device) {
                        return Err(PresentError::Resized);
                    }
                    tracing::trace_span!("wl.copy", from = "texture").in_scope(|| {
                        draw_from(device, programs, &images.buffers[index], texture.texture())
                    })?;
                }
                None => {
                    if images.staging.is_none() {
                        images.staging = Some(
                            device
                                .create_texture(&TextureDesc {
                                    size: allocated,
                                    format: TextureFormat::Rgba8,
                                    usage: TextureUsage::SAMPLED,
                                })
                                .map_err(gpu_error)?,
                        );
                        whole = true;
                    }
                    let staging = images.staging.as_ref().expect("made above");
                    let full = [PixelRect {
                        x: 0,
                        y: 0,
                        width,
                        height,
                    }];
                    let uploads: &[PixelRect] = if whole { &full } else { damage };
                    let _copying =
                        tracing::trace_span!("wl.copy", from = "memory", rects = uploads.len())
                            .entered();
                    let mut packed = Vec::new();
                    for r in uploads {
                        let Some(r) = clip(*r, width, height) else {
                            continue;
                        };
                        pack(frame_pixels, stride, r, &mut packed);
                        device
                            .write_texture(
                                staging,
                                Region::new(r.x, r.y, r.width, r.height),
                                &packed,
                            )
                            .map_err(gpu_error)?;
                    }
                    blit(device, programs, &images.buffers[index], staging)?;
                }
            }
            let fence = match submit {
                Submit::Fence => {
                    let (fence, file) = tracing::trace_span!("wl.submit")
                        .in_scope(|| device.submit_exported())
                        .map_err(gpu_error)?;
                    device.destroy_fence(fence);
                    Some(file)
                }
                Submit::Finish => {
                    tracing::trace_span!("wl.finish")
                        .in_scope(|| device.finish())
                        .map_err(gpu_error)?;
                    None
                }
            };
            Ok(fence)
        })?;
        if source.is_some() {
            self.stats.from_texture += 1;
            // the staging texture missed this frame
            self.stale = true;
            return self.commit(
                gpu,
                conn,
                surface,
                index,
                drawn,
                (pixels, preferred),
                damage,
                shown_whole,
            );
        }
        self.commit(
            gpu,
            conn,
            surface,
            index,
            drawn,
            (pixels, preferred),
            damage,
            whole,
        )?;
        self.stale = false;
        Ok(())
    }

    /// Synchronises the buffer of slot `index`, drawn with a frame of `pixels` and finished with `fence` (the frame's
    /// native fence, `None` once the GPU finished it), and commits it with `damage` (or whole).
    #[allow(clippy::too_many_arguments)]
    fn commit(
        &mut self,
        gpu: &GpuDevice,
        conn: &mut Connection,
        surface: SurfaceId,
        index: usize,
        fence: Option<OwnedFd>,
        (pixels, preferred): ((u32, u32), (u32, u32)),
        damage: &[PixelRect],
        whole: bool,
    ) -> Result<(), PresentError> {
        let _commit = tracing::trace_span!("wl.commit", gpu = true).entered();
        let slot = &mut self.slots[index];
        let sync = match (&gpu.sync, &slot.timeline, fence) {
            (Some(sync), Some((timeline, id)), Some(file)) => {
                slot.uses += 1;
                let (acquire, release) = (2 * slot.uses - 1, 2 * slot.uses);
                sync.signal_with(timeline, file.as_fd(), acquire)
                    .map_err(|e| PresentError::Fallback(format!("acquire point: {e}")))?;
                slot.release = Some(
                    sync.wait_fd(timeline, release)
                        .map_err(|e| PresentError::Fallback(format!("release wait: {e}")))?,
                );
                slot.reuse = Reuse::ReleasePoint;
                Some((*id, acquire, release))
            }
            (_, _, fence) => {
                // without explicit sync the compositor must not read a buffer still being drawn
                if let Some(file) = fence {
                    tracing::trace_span!("wl.fence_wait")
                        .in_scope(|| cupertino_gpu::wait_sync_file(&file, FENCE_WAIT));
                }
                self.stats.fence_waits += 1;
                slot.reuse = Reuse::Release;
                None
            }
        };
        let buffer = slot.buffer;
        conn.present_gpu(
            surface,
            &GpuFrame {
                buffer,
                size: pixels,
                preferred,
                sync,
                damage: (!whole).then_some(damage),
            },
        )?;
        self.commits += 1;
        self.stats.frames += 1;
        Ok(())
    }
}

/// Draws `texture` (a renderer's frame) into `target`, a buffer's texture, texel for texel.
fn draw_from(
    device: &Device,
    programs: &Programs,
    target: &Texture,
    texture: &Texture,
) -> Result<(), PresentError> {
    let group = device
        .create_bind_group(
            &programs.texels_layout,
            &[BindGroupEntry {
                binding: 1,
                resource: BindingResource::Texture(texture),
            }],
        )
        .map_err(gpu_error)?;
    let mut pass = device
        .begin_pass(target, LoadOp::Clear([0.0; 4]))
        .map_err(gpu_error)?;
    pass.set_pipeline(programs.texels());
    pass.set_bind_group(0, &group).map_err(gpu_error)?;
    pass.draw(0..3, 1).map_err(gpu_error)?;
    pass.end();
    Ok(())
}

/// Draws `staging` (uploaded pixels) into `target`, a buffer's texture.
fn blit(
    device: &Device,
    programs: &Programs,
    target: &Texture,
    staging: &Texture,
) -> Result<(), PresentError> {
    let group = device
        .create_bind_group(
            &programs.layout,
            &[
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Texture(staging),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Sampler(programs.sampler()),
                },
            ],
        )
        .map_err(gpu_error)?;
    let mut pass = device
        .begin_pass(target, LoadOp::Clear([0.0; 4]))
        .map_err(gpu_error)?;
    pass.set_pipeline(programs.blit());
    pass.set_bind_group(0, &group).map_err(gpu_error)?;
    pass.draw(0..3, 1).map_err(gpu_error)?;
    pass.end();
    Ok(())
}

/// `r` cut to a `width` × `height` frame; `None` when nothing is left.
fn clip(r: PixelRect, width: u32, height: u32) -> Option<PixelRect> {
    let (x1, y1) = (
        r.x.saturating_add(r.width).min(width),
        r.y.saturating_add(r.height).min(height),
    );
    (r.x < x1 && r.y < y1).then(|| PixelRect {
        x: r.x,
        y: r.y,
        width: x1 - r.x,
        height: y1 - r.y,
    })
}

/// The pixels of `r` (inside the frame) of a frame's `pixels` with rows `stride` bytes apart, rows packed one after
/// the other, into `out`.
fn pack(pixels: &[u8], stride: usize, r: PixelRect, out: &mut Vec<u8>) {
    out.clear();
    let (start, len) = (r.x as usize * 4, r.width as usize * 4);
    for row in r.y..r.y + r.height {
        let at = row as usize * stride + start;
        out.extend_from_slice(&pixels[at..at + len]);
    }
}

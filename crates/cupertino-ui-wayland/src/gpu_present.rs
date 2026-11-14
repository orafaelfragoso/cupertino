//! Presenting a surface's frames from dmabufs drawn on the GPU.
//!
//! A [`GpuSurface`] keeps the buffers of one surface: two with explicit sync (each with a syncobj timeline: the
//! frame's native fence becomes its acquire point, and the buffer is drawn into again once its release point
//! signalled), three without (the client waits for its own fence before it commits, and draws into a buffer again only
//! after the compositor released it and one further frame went out). Buffers are allocated in size steps
//! ([`allocation`]) and cropped to the frame by the surface's viewport, so a live resize allocates once a step, not
//! once a frame. A buffer uses only a modifier the compositor offers and the driver renders to; linear is never added
//! on a hardware driver ([`usable_modifiers`]).
//!
//! The drawing itself: a frame drawn on the same device ([`cupertino_ui_gpu::WindowRenderer`]) is copied from its
//! texture into the buffer, texel for texel from the first pixel (the buffer may be larger by the size step), and
//! nothing is uploaded. A frame of the CPU renderer has its pixels uploaded into a texture of the buffer's size, only
//! what the frame changed, and drawn over the buffer; a frame drawn on another device is read back first.

use std::os::fd::{AsFd, OwnedFd};
use std::rc::Rc;
use std::time::Duration;

use cupertino_gpu::syncobj::{self, SyncNode, Timeline};
use cupertino_gpu::{
    shaders, AddressMode, BindGroupEntry, BindGroupLayout, BindingKind, BindingLayout,
    BindingResource, Blend, Device, DmabufDesc, Error as GpuError, ExternalImage, Filter, LoadOp,
    Loss, Pipeline, PipelineDesc, Region, Sampler, SamplerDesc, Texture, TextureDesc,
    TextureFormat, TextureUsage, Topology, FOURCC_ARGB8888, MODIFIER_INVALID, MODIFIER_LINEAR,
};
use cupertino_ui_gpu::{GpuPrograms, WindowTexture};
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

/// The process's GPU device and what every surface draws with. The device is shared with the windows' renderers.
pub struct GpuDevice {
    pub device: Rc<Device>,
    /// The render node's syncobj requests, when explicit sync is used.
    sync: Option<SyncNode>,
    /// The modifiers buffers may use.
    modifiers: Vec<u64>,
    blit: Pipeline,
    layout: BindGroupLayout,
    sampler: Sampler,
    texels: Pipeline,
    texels_layout: BindGroupLayout,
    /// The windows' renderers' programs.
    pub ui: Rc<GpuPrograms>,
}

/// The programs a surface draws with and those of the windows' renderers, made when the device is (on the warm-up
/// thread).
pub struct Programs {
    pub blit: Pipeline,
    pub layout: BindGroupLayout,
    pub sampler: Sampler,
    /// A renderer's texture copied into a buffer texel for texel.
    pub texels: Pipeline,
    pub texels_layout: BindGroupLayout,
    pub ui: GpuPrograms,
}

impl Programs {
    pub fn new(device: &Device) -> Result<Programs, GpuError> {
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
            blit,
            layout,
            sampler,
            texels,
            texels_layout,
            ui: GpuPrograms::new(device)?,
        })
    }
}

impl GpuDevice {
    /// The process's device with its programs, presenting buffers in modifiers of `offered` (ARGB8888's in the
    /// compositor's feedback), with explicit sync through `sync` when given.
    pub fn new(
        device: Device,
        programs: Programs,
        offered: &[u64],
        sync: Option<SyncNode>,
    ) -> GpuDevice {
        let info = device.info().clone();
        let modifiers = match info.allocator {
            // the export allocator takes the driver's layout, which the offer must hold
            Some(cupertino_gpu::Allocator::Export) => offered.to_vec(),
            _ => usable_modifiers(
                offered,
                &device.renderable_modifiers(FOURCC_ARGB8888),
                info.software,
            ),
        };
        let sync = sync.filter(|_| info.native_fences);
        GpuDevice {
            device: Rc::new(device),
            sync,
            modifiers,
            blit: programs.blit,
            layout: programs.layout,
            sampler: programs.sampler,
            texels: programs.texels,
            texels_layout: programs.texels_layout,
            ui: Rc::new(programs.ui),
        }
    }

    /// Whether buffers are synchronised through syncobj timelines.
    pub fn explicit(&self) -> bool {
        self.sync.is_some()
    }

    pub fn modifiers(&self) -> &[u64] {
        &self.modifiers
    }
}

struct Slot {
    image: ExternalImage,
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

/// The GPU buffers of one surface.
#[derive(Default)]
pub struct GpuSurface {
    slots: Vec<Slot>,
    staging: Option<Texture>,
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
                Reuse::Release if !conn.gpu_buffer_busy(slot.buffer) => {
                    slot.reuse = Reuse::ReleasedAt(commits);
                }
                _ => {}
            }
        }
    }

    /// Whether a frame could be drawn now: a buffer is free or another may be allocated.
    pub fn has_free_buffer(&mut self, gpu: &GpuDevice, conn: &Connection) -> bool {
        self.refresh(gpu, conn);
        let commits = self.commits;
        self.slots.len() < buffer_count(gpu.explicit())
            || self.slots.iter().any(|s| s.reuse.free(commits))
    }

    /// The staging texture no longer holds the last frame (after a purge): the next present uploads all of it.
    pub fn forget(&mut self) {
        self.stale = true;
    }

    /// Free every buffer, texture and timeline.
    pub fn free(&mut self, gpu: Option<&GpuDevice>, conn: &mut Connection) {
        for slot in self.slots.drain(..) {
            conn.destroy_gpu_buffer(slot.buffer);
            if let Some((timeline, id)) = slot.timeline {
                conn.destroy_timeline(id);
                if let Some(sync) = gpu.and_then(|g| g.sync.as_ref()) {
                    sync.destroy(timeline);
                }
            }
            if let Some(gpu) = gpu {
                gpu.device.destroy_texture(slot.image.texture);
            }
        }
        if let (Some(t), Some(gpu)) = (self.staging.take(), gpu) {
            gpu.device.destroy_texture(t);
        }
        self.staging = None;
        self.allocated = None;
    }

    fn allocate(
        &mut self,
        gpu: &GpuDevice,
        conn: &mut Connection,
        size: (u32, u32),
    ) -> Result<(), PresentError> {
        let image = gpu
            .device
            .allocate_dmabuf(&DmabufDesc {
                size,
                fourcc: FOURCC_ARGB8888,
                modifiers: gpu.modifiers.clone(),
            })
            .map_err(gpu_error)?;
        let planes = image
            .dmabuf
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
            fourcc: image.dmabuf.fourcc,
            modifier: image.dmabuf.modifier,
            planes,
        });
        let buffer = match made {
            Ok(b) => b,
            Err(e) => {
                gpu.device.destroy_texture(image.texture);
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
                        gpu.device.destroy_texture(image.texture);
                        return Err(PresentError::Fallback(format!("syncobj: {e}")));
                    }
                }
            }
            None => None,
        };
        self.stats.allocations += 1;
        self.stats.modifier = image.dmabuf.modifier;
        self.slots.push(Slot {
            image,
            buffer,
            timeline,
            uses: 0,
            reuse: Reuse::Free,
            release: None,
        });
        Ok(())
    }

    /// Show `frame` on `surface`: copied from its renderer's texture on this device, or its `damage` (pixels;
    /// everything when `whole`) uploaded, drawn into a free buffer, synchronised and committed.
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
        gpu.device.status().map_err(gpu_error)?;
        let size = conn.frame_size(surface, preferred)?;
        let pixels = (frame.width, frame.height);
        if (size.width, size.height) != pixels {
            return Err(PresentError::Resized);
        }
        // a frame drawn on this device is copied from its texture; one drawn on another is read back
        let source = frame
            .device
            .as_ref()
            .and_then(|d| d.0.as_any().downcast_ref::<WindowTexture>())
            .filter(|w| Rc::ptr_eq(w.device(), &gpu.device) && w.size() == pixels);
        let read_back;
        let frame = if source.is_none() && frame.pixels.is_empty() {
            // a frame of a device given up (a loss) is not shown: the next comes from a renderer of this one
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
            let max = gpu.device.info().max_texture_size;
            self.allocated = Some(allocation(pixels, can_crop, max));
            whole = true;
            shown_whole = true;
        }
        let allocated = self.allocated.expect("set above");
        self.stats.allocated = allocated;
        self.refresh(gpu, conn);
        let commits = self.commits;
        let index = match self.slots.iter().position(|s| s.reuse.free(commits)) {
            Some(i) => i,
            None if self.slots.len() < buffer_count(gpu.explicit()) => {
                self.allocate(gpu, conn, allocated)?;
                self.slots.len() - 1
            }
            None => return Err(PresentError::Busy),
        };
        if let Some(source) = source {
            self.draw_from(gpu, index, source.texture())?;
            self.stats.from_texture += 1;
            // the staging texture missed this frame
            self.stale = true;
            return self.commit(
                gpu,
                conn,
                surface,
                index,
                (pixels, preferred),
                damage,
                shown_whole,
            );
        }
        if self.staging.is_none() {
            self.staging = Some(
                gpu.device
                    .create_texture(&TextureDesc {
                        size: allocated,
                        format: TextureFormat::Rgba8,
                        usage: TextureUsage::SAMPLED,
                    })
                    .map_err(gpu_error)?,
            );
            whole = true;
        }
        let staging = self.staging.as_ref().expect("made above");
        let full = [PixelRect {
            x: 0,
            y: 0,
            width: frame.width,
            height: frame.height,
        }];
        let uploads: &[PixelRect] = if whole { &full } else { damage };
        let mut packed = Vec::new();
        for r in uploads {
            let Some(r) = clip(*r, frame.width, frame.height) else {
                continue;
            };
            pack(frame, r, &mut packed);
            gpu.device
                .write_texture(staging, Region::new(r.x, r.y, r.width, r.height), &packed)
                .map_err(gpu_error)?;
        }
        let slot = &self.slots[index];
        let group = gpu
            .device
            .create_bind_group(
                &gpu.layout,
                &[
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Texture(staging),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: BindingResource::Sampler(&gpu.sampler),
                    },
                ],
            )
            .map_err(gpu_error)?;
        {
            let mut pass = gpu
                .device
                .begin_pass(&slot.image.texture, LoadOp::Clear([0.0; 4]))
                .map_err(gpu_error)?;
            pass.set_pipeline(&gpu.blit);
            pass.set_bind_group(0, &group).map_err(gpu_error)?;
            pass.draw(0..3, 1).map_err(gpu_error)?;
            pass.end();
        }
        self.commit(
            gpu,
            conn,
            surface,
            index,
            (pixels, preferred),
            damage,
            whole,
        )?;
        self.stale = false;
        Ok(())
    }

    /// Draws `texture` (a renderer's frame) into the buffer of slot `index`, texel for texel.
    fn draw_from(
        &self,
        gpu: &GpuDevice,
        index: usize,
        texture: &Texture,
    ) -> Result<(), PresentError> {
        let group = gpu
            .device
            .create_bind_group(
                &gpu.texels_layout,
                &[BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Texture(texture),
                }],
            )
            .map_err(gpu_error)?;
        let mut pass = gpu
            .device
            .begin_pass(&self.slots[index].image.texture, LoadOp::Clear([0.0; 4]))
            .map_err(gpu_error)?;
        pass.set_pipeline(&gpu.texels);
        pass.set_bind_group(0, &group).map_err(gpu_error)?;
        pass.draw(0..3, 1).map_err(gpu_error)?;
        pass.end();
        Ok(())
    }

    /// Synchronises the buffer of slot `index`, drawn with a frame of `pixels`, and commits it with `damage` (or
    /// whole).
    #[allow(clippy::too_many_arguments)]
    fn commit(
        &mut self,
        gpu: &GpuDevice,
        conn: &mut Connection,
        surface: SurfaceId,
        index: usize,
        (pixels, preferred): ((u32, u32), (u32, u32)),
        damage: &[PixelRect],
        whole: bool,
    ) -> Result<(), PresentError> {
        let slot = &mut self.slots[index];
        let sync = match (&gpu.sync, &slot.timeline) {
            (Some(sync), Some((timeline, id))) => {
                let (fence, file) = gpu.device.submit_exported().map_err(gpu_error)?;
                gpu.device.destroy_fence(fence);
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
            _ => {
                // without explicit sync the compositor must not read a buffer still being drawn
                if gpu.device.info().native_fences {
                    let (fence, file) = gpu.device.submit_exported().map_err(gpu_error)?;
                    gpu.device.destroy_fence(fence);
                    cupertino_gpu::wait_sync_file(&file, FENCE_WAIT);
                } else {
                    gpu.device.finish().map_err(gpu_error)?;
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

/// The pixels of `r` (inside the frame) of `frame`, rows packed one after the other, into `out`.
fn pack(frame: &CpuFrame, r: PixelRect, out: &mut Vec<u8>) {
    out.clear();
    let stride = frame.stride as usize;
    let (start, len) = (r.x as usize * 4, r.width as usize * 4);
    for row in r.y..r.y + r.height {
        let at = row as usize * stride + start;
        out.extend_from_slice(&frame.pixels[at..at + len]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NVIDIA_DEFAULT: u64 = 0x0300_0000_00e0_8014;

    #[test]
    fn two_buffers_with_release_points_and_three_without() {
        assert_eq!(buffer_count(true), 2);
        assert_eq!(buffer_count(false), 3);
    }

    #[test]
    fn a_resize_allocates_once_a_step_and_never_more_than_two_steps_too_large() {
        assert_eq!(allocation((301, 199), true, 16384), (384, 256));
        assert_eq!(allocation((301, 199), false, 16384), (301, 199));
        assert_eq!(allocation((16380, 10), true, 16384), (16384, 128));
        // a live resize of one pixel a frame from 300 to 700 pixels wide allocates once a step it crosses
        let mut allocated = allocation((300, 200), true, 16384);
        let mut allocations = 1;
        for w in 301..=700 {
            if !serves(allocated, (w, 200), true) {
                allocated = allocation((w, 200), true, 16384);
                allocations += 1;
            }
        }
        assert_eq!(allocations, 4);
        // shrinking keeps the buffers until they are two steps too large
        assert!(serves((768, 256), (641, 200), true));
        assert!(!serves((768, 256), (512, 200), true));
        // without a viewport the buffer is the frame's size exactly
        assert!(!serves((384, 256), (301, 199), false));
        assert!(serves((301, 199), (301, 199), false));
    }

    #[test]
    fn a_hardware_buffer_never_gets_linear_unless_its_driver_renders_to_it() {
        let offered = [MODIFIER_LINEAR, NVIDIA_DEFAULT, MODIFIER_INVALID];
        assert_eq!(
            usable_modifiers(&offered, &[NVIDIA_DEFAULT], false),
            [NVIDIA_DEFAULT, MODIFIER_INVALID]
        );
        assert_eq!(
            usable_modifiers(&[MODIFIER_LINEAR], &[MODIFIER_LINEAR], false),
            [MODIFIER_LINEAR]
        );
        assert!(usable_modifiers(&[NVIDIA_DEFAULT], &[MODIFIER_LINEAR], false).is_empty());
        assert_eq!(
            usable_modifiers(&[MODIFIER_LINEAR, NVIDIA_DEFAULT], &[MODIFIER_LINEAR], true),
            [MODIFIER_LINEAR]
        );
    }

    #[test]
    fn without_explicit_sync_a_released_buffer_waits_one_more_commit() {
        assert!(Reuse::Free.free(0));
        assert!(!Reuse::Release.free(10));
        assert!(!Reuse::ReleasePoint.free(10));
        assert!(!Reuse::ReleasedAt(5).free(5));
        assert!(Reuse::ReleasedAt(5).free(6));
    }

    #[test]
    fn an_upload_packs_the_rows_of_its_rectangle() {
        let frame = CpuFrame {
            id: cupertino_ui_render::RenderFrameId(1),
            width: 3,
            height: 2,
            stride: 12,
            format: cupertino_ui_render::PixelFormat::Rgba8Premultiplied,
            pixels: (0..24).collect(),
            damage: Vec::new(),
            device: None,
        };
        let mut out = Vec::new();
        pack(
            &frame,
            PixelRect {
                x: 1,
                y: 0,
                width: 2,
                height: 2,
            },
            &mut out,
        );
        assert_eq!(
            out,
            [4, 5, 6, 7, 8, 9, 10, 11, 16, 17, 18, 19, 20, 21, 22, 23]
        );
        assert_eq!(
            clip(
                PixelRect {
                    x: 2,
                    y: 1,
                    width: 5,
                    height: 5
                },
                3,
                2
            ),
            Some(PixelRect {
                x: 2,
                y: 1,
                width: 1,
                height: 1
            })
        );
    }
}

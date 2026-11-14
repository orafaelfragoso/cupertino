//! Buffers a client draws on a GPU: what the compositor's dmabuf feedback names (the device and the formats with their
//! modifiers), `wl_buffer`s of dmabufs (`zwp_linux_dmabuf_v1`, made with `create` so a buffer the compositor cannot
//! import is an answer, not a protocol error), syncobj timelines and the per-surface syncobj object of
//! `linux-drm-syncobj-v1`, and presenting such a buffer with its acquire and release points. The global is bound on
//! first use, so a process that draws on the CPU never binds it. A surface's syncobj object is destroyed before it shows
//! a shared-memory buffer again, since a commit without points on a surface that has one is a protocol error.

use std::os::fd::{BorrowedFd, OwnedFd};

use wayland_client::backend::ObjectData;
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::{Connection as WlConnection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::{
    self, ZwpLinuxBufferParamsV1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_feedback_v1::{
    self, ZwpLinuxDmabufFeedbackV1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::{
    self, ZwpLinuxDmabufV1,
};
use wayland_protocols::wp::linux_drm_syncobj::v1::client::wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1;
use wayland_protocols::wp::linux_drm_syncobj::v1::client::wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1;
use wayland_protocols::wp::linux_drm_syncobj::v1::client::wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1;

use super::{clamp_i32, logical_size, Connection, Role, State, Surface, SurfaceId};
use crate::{Error, PixelRect, Scale};

/// What the compositor's dmabuf feedback says: the device it composites on and the formats it imports.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DmabufFeedback {
    /// The device the compositor composites on (a `dev_t`); `None` from a compositor without feedback (version 3).
    pub main_device: Option<u64>,
    /// The (format, modifier) pairs it imports, the preferred tranche's first.
    pub formats: Vec<(u32, u64)>,
}

impl DmabufFeedback {
    /// The modifiers offered for `fourcc`, in the compositor's order.
    pub fn modifiers(&self, fourcc: u32) -> Vec<u64> {
        let mut out = Vec::new();
        for &(f, m) in &self.formats {
            if f == fourcc && !out.contains(&m) {
                out.push(m);
            }
        }
        out
    }
}

/// One plane of a dmabuf to make a buffer of.
#[derive(Clone, Copy, Debug)]
pub struct DmabufPlaneRef<'a> {
    pub fd: BorrowedFd<'a>,
    pub offset: u32,
    pub stride: u32,
}

/// A dmabuf to make a `wl_buffer` of: pixel size, DRM format, modifier and planes.
#[derive(Clone, Debug)]
pub struct DmabufAttributes<'a> {
    pub size: (u32, u32),
    pub fourcc: u32,
    pub modifier: u64,
    pub planes: Vec<DmabufPlaneRef<'a>>,
}

/// A dmabuf buffer of the connection ([`Connection::create_dmabuf_buffer`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GpuBufferId(u64);

/// A syncobj timeline the compositor imported ([`Connection::import_timeline`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TimelineId(u64);

/// A frame of a dmabuf buffer to show ([`Connection::present_gpu`]).
#[derive(Clone, Copy, Debug)]
pub struct GpuFrame<'a> {
    pub buffer: GpuBufferId,
    /// The pixels of the buffer the frame fills from its top left corner, shown over the surface; a buffer larger than
    /// the frame (allocated in size steps during a live resize) is cropped to them.
    pub size: (u32, u32),
    /// Acquire and release points on a timeline; `None` without explicit sync (the frame's own fence was waited for).
    pub sync: Option<(TimelineId, u64, u64)>,
    /// What changed since the frame shown last, in buffer pixels; `None` for all of it.
    pub damage: Option<&'a [PixelRect]>,
}

/// The logical size and scale a frame of a surface is drawn at, and its pixel size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameSize {
    pub logical: (u32, u32),
    pub scale: Scale,
    pub width: u32,
    pub height: u32,
}

struct GpuBuffer {
    id: u64,
    wl: WlBuffer,
    size: (u32, u32),
    busy: bool,
}

#[derive(Default)]
struct FeedbackState {
    table: Vec<(u32, u64)>,
    main_device: Option<u64>,
    tranche: Vec<u16>,
    tranches: Vec<Vec<u16>>,
    done: bool,
}

/// How a `create` went: waiting, made, or refused.
enum Params {
    Waiting,
    Created(WlBuffer),
    Failed,
}

/// The connection's GPU buffer state.
#[derive(Default)]
pub(crate) struct GpuState {
    bound: bool,
    dmabuf: Option<ZwpLinuxDmabufV1>,
    syncobj: Option<WpLinuxDrmSyncobjManagerV1>,
    v3_formats: Vec<(u32, u64)>,
    feedback: FeedbackState,
    feedback_result: Option<DmabufFeedback>,
    buffers: Vec<GpuBuffer>,
    timelines: Vec<(u64, WpLinuxDrmSyncobjTimelineV1)>,
    params: Vec<(u64, Params)>,
    next: u64,
    /// Syncobj surface objects made and destroyed, in all.
    pub(crate) syncobj_surfaces: (u64, u64),
}

/// User data of a params object: the id its buffer gets.
pub(crate) struct ParamsTag(u64);
/// User data of a dmabuf `wl_buffer`.
pub(crate) struct DmabufBuffer;
/// User data of the objects that receive no event this transport reads.
pub(crate) struct GpuQuiet;

impl Surface {
    /// Destroy the syncobj object before the surface shows a shared-memory buffer or one pixel of a colour.
    pub(crate) fn drop_syncobj(&mut self, gpu: &mut GpuState) {
        if let Some(s) = self.syncobj.take() {
            s.destroy();
            gpu.syncobj_surfaces.1 += 1;
        }
    }
}

impl GpuState {
    fn bind(&mut self, globals: &wayland_client::globals::GlobalList, qh: &QueueHandle<State>) {
        if self.bound {
            return;
        }
        self.bound = true;
        self.dmabuf = globals.bind(qh, 3..=4, GpuQuiet).ok();
        self.syncobj = globals.bind(qh, 1..=1, GpuQuiet).ok();
    }
}

impl Connection {
    fn bind_gpu(&mut self) -> Result<(), Error> {
        if self.state.gpu.bound {
            return Ok(());
        }
        let qh = self.queue.handle();
        self.state.gpu.bind(&self.globals, &qh);
        // a version 3 global sends its formats and modifiers right after the bind
        if self
            .state
            .gpu
            .dmabuf
            .as_ref()
            .is_some_and(|d| d.version() < 4)
        {
            self.roundtrip()?;
        }
        Ok(())
    }

    /// The version of `zwp_linux_dmabuf_v1` the compositor offers (binding it), `None` without one.
    pub fn dmabuf_version(&mut self) -> Result<Option<u32>, Error> {
        self.bind_gpu()?;
        Ok(self.state.gpu.dmabuf.as_ref().map(Proxy::version))
    }

    /// Whether the compositor offers explicit synchronisation (`wp_linux_drm_syncobj_manager_v1`), binding it.
    pub fn has_explicit_sync(&mut self) -> Result<bool, Error> {
        self.bind_gpu()?;
        Ok(self.state.gpu.syncobj.is_some())
    }

    /// The compositor's default dmabuf feedback, read once (a round trip the first time); `None` when it offers no
    /// dmabuf protocol. From a compositor of version 3 it has no main device.
    pub fn dmabuf_feedback(&mut self) -> Result<Option<DmabufFeedback>, Error> {
        self.bind_gpu()?;
        let Some(dmabuf) = self.state.gpu.dmabuf.clone() else {
            return Ok(None);
        };
        if let Some(fb) = &self.state.gpu.feedback_result {
            return Ok(Some(fb.clone()));
        }
        let result = if dmabuf.version() >= 4 {
            let feedback = dmabuf.get_default_feedback(&self.queue.handle(), GpuQuiet);
            while !self.state.gpu.feedback.done {
                self.roundtrip()?;
            }
            feedback.destroy();
            let fb = std::mem::take(&mut self.state.gpu.feedback);
            let mut formats = Vec::new();
            for tranche in &fb.tranches {
                for &i in tranche {
                    if let Some(&pair) = fb.table.get(usize::from(i)) {
                        if !formats.contains(&pair) {
                            formats.push(pair);
                        }
                    }
                }
            }
            DmabufFeedback {
                main_device: fb.main_device,
                formats,
            }
        } else {
            DmabufFeedback {
                main_device: None,
                formats: self.state.gpu.v3_formats.clone(),
            }
        };
        self.state.gpu.feedback_result = Some(result.clone());
        Ok(Some(result))
    }

    /// A `wl_buffer` of the dmabuf `attrs`, waiting for the compositor's answer: [`Error::BufferRejected`] when it
    /// cannot import it (the surface falls back to shared memory), [`Error::Unsupported`] without the protocol.
    pub fn create_dmabuf_buffer(
        &mut self,
        attrs: &DmabufAttributes<'_>,
    ) -> Result<GpuBufferId, Error> {
        self.bind_gpu()?;
        let dmabuf = self
            .state
            .gpu
            .dmabuf
            .clone()
            .ok_or(Error::Unsupported(crate::Capability::Dmabuf))?;
        let qh = self.queue.handle();
        let id = self.state.gpu.next;
        self.state.gpu.next += 1;
        let params = dmabuf.create_params(&qh, ParamsTag(id));
        for (i, plane) in attrs.planes.iter().enumerate() {
            params.add(
                plane.fd,
                i as u32,
                plane.offset,
                plane.stride,
                (attrs.modifier >> 32) as u32,
                attrs.modifier as u32,
            );
        }
        self.state.gpu.params.push((id, Params::Waiting));
        params.create(
            clamp_i32(attrs.size.0),
            clamp_i32(attrs.size.1),
            attrs.fourcc,
            zwp_linux_buffer_params_v1::Flags::empty(),
        );
        let outcome = loop {
            let at = self
                .state
                .gpu
                .params
                .iter()
                .position(|(p, _)| *p == id)
                .expect("pushed above");
            if !matches!(self.state.gpu.params[at].1, Params::Waiting) {
                break self.state.gpu.params.remove(at).1;
            }
            self.roundtrip()?;
        };
        params.destroy();
        match outcome {
            Params::Created(wl) => {
                self.state.gpu.buffers.push(GpuBuffer {
                    id,
                    wl,
                    size: attrs.size,
                    busy: false,
                });
                Ok(GpuBufferId(id))
            }
            _ => Err(Error::BufferRejected),
        }
    }

    /// Destroy a dmabuf buffer; the compositor may still show it until the surface commits another.
    pub fn destroy_gpu_buffer(&mut self, buffer: GpuBufferId) {
        if let Some(i) = self.state.gpu.buffers.iter().position(|b| b.id == buffer.0) {
            self.state.gpu.buffers.remove(i).wl.destroy();
        }
    }

    /// Whether the compositor still holds `buffer` (from its present until its `wl_buffer.release`).
    pub fn gpu_buffer_busy(&self, buffer: GpuBufferId) -> bool {
        self.state
            .gpu
            .buffers
            .iter()
            .any(|b| b.id == buffer.0 && b.busy)
    }

    /// Hand the compositor a syncobj timeline (`import_timeline`), as a file descriptor of the client's render node.
    pub fn import_timeline(&mut self, fd: BorrowedFd<'_>) -> Result<TimelineId, Error> {
        self.bind_gpu()?;
        let manager = self
            .state
            .gpu
            .syncobj
            .clone()
            .ok_or(Error::Unsupported(crate::Capability::ExplicitSync))?;
        let id = self.state.gpu.next;
        self.state.gpu.next += 1;
        let timeline = manager.import_timeline(fd, &self.queue.handle(), GpuQuiet);
        self.state.gpu.timelines.push((id, timeline));
        Ok(TimelineId(id))
    }

    /// Destroy a timeline the compositor imported.
    pub fn destroy_timeline(&mut self, timeline: TimelineId) {
        if let Some(i) = self
            .state
            .gpu
            .timelines
            .iter()
            .position(|(t, _)| *t == timeline.0)
        {
            self.state.gpu.timelines.remove(i).1.destroy();
        }
    }

    /// The syncobj surface objects made and destroyed since the connection started.
    pub fn syncobj_surfaces(&self) -> (u64, u64) {
        self.state.gpu.syncobj_surfaces
    }

    /// Whether `surface` has a syncobj surface object now.
    pub fn has_syncobj_surface(&self, surface: SurfaceId) -> Result<bool, Error> {
        Ok(self.state.surface(surface)?.syncobj.is_some())
    }

    /// The size a frame of `surface` is drawn at: the configured logical size (or `preferred` where the compositor
    /// leaves it to the client), the surface's scale and the pixel size.
    pub fn frame_size(
        &self,
        surface: SurfaceId,
        preferred: (u32, u32),
    ) -> Result<FrameSize, Error> {
        let s = &self.state;
        let parent_scale = s.drawn_scale(surface)?;
        let surf = s.surface(surface)?;
        if !matches!(surf.role, Role::Subsurface { .. }) {
            surf.serials.latest().ok_or(Error::NotConfigured)?;
        }
        let logical = logical_size(&surf.role, preferred)?;
        let scale = parent_scale.unwrap_or(surf.scale);
        let (width, height) = scale
            .to_physical(logical.0)
            .zip(scale.to_physical(logical.1))
            .filter(|&(w, h)| w > 0 && h > 0)
            .ok_or(Error::InvalidScale)?;
        Ok(FrameSize {
            logical,
            scale,
            width,
            height,
        })
    }

    /// Whether the surface can show a buffer cropped to a frame smaller than it (it has a viewport), which allocating
    /// in size steps needs.
    pub fn can_crop(&self, surface: SurfaceId) -> Result<bool, Error> {
        Ok(self.state.surface(surface)?.viewport.is_some())
    }

    /// Show `frame` on `surface`, answering its latest configure, with a frame callback and presentation feedback.
    /// With points the surface gets its syncobj object first (made once, kept while it shows dmabufs); without, any
    /// object it had is destroyed.
    pub fn present_gpu(&mut self, surface: SurfaceId, frame: &GpuFrame<'_>) -> Result<(), Error> {
        let qh = self.queue.handle();
        let presentation = self.state.presentation.clone();
        let size = self.frame_size(surface, frame.size)?;
        let State { surfaces, gpu, .. } = &mut self.state;
        let surf = surfaces
            .iter_mut()
            .find(|s| s.id == surface)
            .ok_or(Error::UnknownSurface)?;
        let at = gpu
            .buffers
            .iter()
            .position(|b| b.id == frame.buffer.0)
            .ok_or(Error::Protocol("present_gpu of a destroyed buffer"))?;
        let buffer_size = gpu.buffers[at].size;
        if frame.size.0 > buffer_size.0 || frame.size.1 > buffer_size.1 {
            return Err(Error::Protocol("a frame larger than its buffer"));
        }
        let subsurface = matches!(surf.role, Role::Subsurface { .. });
        if !subsurface {
            if let Some(serial) = surf.serials.present()? {
                match &surf.role {
                    Role::Toplevel { xdg, .. } | Role::Popup { xdg, .. } => {
                        xdg.ack_configure(serial)
                    }
                    Role::External { role, .. } => role.ack_configure(serial),
                    Role::Subsurface { .. } => {}
                }
            }
        }
        match frame.sync {
            Some((timeline, acquire, release)) => {
                let manager = gpu
                    .syncobj
                    .clone()
                    .ok_or(Error::Unsupported(crate::Capability::ExplicitSync))?;
                let timeline = gpu
                    .timelines
                    .iter()
                    .find(|(t, _)| *t == timeline.0)
                    .map(|(_, t)| t.clone())
                    .ok_or(Error::Protocol("present_gpu on a destroyed timeline"))?;
                if surf.syncobj.is_none() {
                    surf.syncobj = Some(manager.get_surface(&surf.wl, &qh, GpuQuiet));
                    gpu.syncobj_surfaces.0 += 1;
                }
                let object = surf.syncobj.as_ref().expect("made above");
                object.set_acquire_point(&timeline, (acquire >> 32) as u32, acquire as u32);
                object.set_release_point(&timeline, (release >> 32) as u32, release as u32);
            }
            None => surf.drop_syncobj(gpu),
        }
        // the shared-memory slots no longer hold what is shown
        for slot in &mut surf.slots {
            slot.missed.forget();
        }
        let buffer = &mut gpu.buffers[at];
        buffer.busy = true;
        surf.wl.attach(Some(&buffer.wl), 0, 0);
        let cropped = frame.size != buffer_size;
        match (&surf.viewport, size.scale.integer()) {
            (Some(viewport), _) => {
                surf.wl.set_buffer_scale(1);
                if cropped {
                    viewport.set_source(0.0, 0.0, f64::from(frame.size.0), f64::from(frame.size.1));
                    surf.zoomed = true;
                } else if surf.zoomed {
                    viewport.set_source(-1.0, -1.0, -1.0, -1.0);
                    surf.zoomed = false;
                }
                viewport.set_destination(clamp_i32(size.logical.0), clamp_i32(size.logical.1));
            }
            (None, _) if cropped => {
                return Err(Error::Protocol(
                    "a cropped frame on a surface without a viewport",
                ))
            }
            (None, Some(n)) => surf.wl.set_buffer_scale(n),
            (None, None) => return Err(Error::InvalidScale),
        }
        match frame.damage {
            None => surf.wl.damage_buffer(0, 0, i32::MAX, i32::MAX),
            Some(rects) => {
                for r in rects {
                    surf.wl.damage_buffer(
                        clamp_i32(r.x),
                        clamp_i32(r.y),
                        clamp_i32(r.width),
                        clamp_i32(r.height),
                    );
                }
            }
        }
        if !subsurface {
            surf.wl.frame(&qh, surface);
            if let Some(presentation) = presentation {
                presentation.feedback(&surf.wl, &qh, surface);
            }
        }
        surf.wl.commit();
        self.flush()
    }
}

fn dev_of(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_ne_bytes(bytes.get(..8)?.try_into().ok()?))
}

/// The format table: 16-byte entries of a format, padding and a modifier.
fn read_table(fd: OwnedFd, size: usize) -> Vec<(u32, u64)> {
    if size == 0 {
        return Vec::new();
    }
    let file = std::fs::File::from(fd);
    // SAFETY: a read-only private mapping of the table the compositor sent, dropped before returning; the
    // compositor never changes it (the protocol makes it immutable).
    let Ok(map) = (unsafe {
        memmap2::MmapOptions::new()
            .len(size)
            .map_copy_read_only(&file)
    }) else {
        return Vec::new();
    };
    map.as_chunks::<16>()
        .0
        .iter()
        .map(|e| {
            (
                u32::from_ne_bytes(e[0..4].try_into().expect("4 bytes")),
                u64::from_ne_bytes(e[8..16].try_into().expect("8 bytes")),
            )
        })
        .collect()
}

impl Dispatch<ZwpLinuxDmabufV1, GpuQuiet> for State {
    fn event(
        state: &mut Self,
        _: &ZwpLinuxDmabufV1,
        event: zwp_linux_dmabuf_v1::Event,
        _: &GpuQuiet,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_linux_dmabuf_v1::Event::Modifier {
            format,
            modifier_hi,
            modifier_lo,
        } = event
        {
            let pair = (
                format,
                (u64::from(modifier_hi) << 32) | u64::from(modifier_lo),
            );
            if !state.gpu.v3_formats.contains(&pair) {
                state.gpu.v3_formats.push(pair);
            }
        }
    }
}

impl Dispatch<ZwpLinuxDmabufFeedbackV1, GpuQuiet> for State {
    fn event(
        state: &mut Self,
        _: &ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        _: &GpuQuiet,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_linux_dmabuf_feedback_v1::Event;
        let fb = &mut state.gpu.feedback;
        match event {
            Event::FormatTable { fd, size } => fb.table = read_table(fd, size as usize),
            Event::MainDevice { device } => fb.main_device = dev_of(&device),
            Event::TrancheFormats { indices } => fb.tranche.extend(
                indices
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_ne_bytes(*c)),
            ),
            Event::TrancheDone => {
                let tranche = std::mem::take(&mut fb.tranche);
                fb.tranches.push(tranche);
            }
            Event::Done => fb.done = true,
            _ => {}
        }
    }
}

impl Dispatch<ZwpLinuxBufferParamsV1, ParamsTag> for State {
    fn event(
        state: &mut Self,
        _: &ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        tag: &ParamsTag,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let outcome = match event {
            zwp_linux_buffer_params_v1::Event::Created { buffer } => Params::Created(buffer),
            zwp_linux_buffer_params_v1::Event::Failed => Params::Failed,
            _ => return,
        };
        if let Some((_, p)) = state.gpu.params.iter_mut().find(|(id, _)| *id == tag.0) {
            *p = outcome;
        }
    }

    fn event_created_child(opcode: u16, qh: &QueueHandle<Self>) -> std::sync::Arc<dyn ObjectData> {
        match opcode {
            zwp_linux_buffer_params_v1::EVT_CREATED_OPCODE => {
                qh.make_data::<WlBuffer, _>(DmabufBuffer)
            }
            _ => panic!("zwp_linux_buffer_params_v1 makes no other object"),
        }
    }
}

impl Dispatch<WlBuffer, DmabufBuffer> for State {
    fn event(
        state: &mut Self,
        buffer: &WlBuffer,
        event: wl_buffer::Event,
        _: &DmabufBuffer,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            if let Some(b) = state.gpu.buffers.iter_mut().find(|b| b.wl == *buffer) {
                b.busy = false;
            }
        }
    }
}

macro_rules! quiet {
    ($($interface:ty),*) => {$(
        impl Dispatch<$interface, GpuQuiet> for State {
            fn event(
                _: &mut State,
                _: &$interface,
                _: <$interface as Proxy>::Event,
                _: &GpuQuiet,
                _: &WlConnection,
                _: &QueueHandle<State>,
            ) {
            }
        }
    )*};
}

quiet!(
    WpLinuxDrmSyncobjManagerV1,
    WpLinuxDrmSyncobjTimelineV1,
    WpLinuxDrmSyncobjSurfaceV1
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;

    #[test]
    fn the_modifiers_of_a_format_come_in_the_compositor_s_order_once_each() {
        let fb = DmabufFeedback {
            main_device: Some(1),
            formats: vec![(1, 7), (2, 9), (1, 0), (1, 7)],
        };
        assert_eq!(fb.modifiers(1), [7, 0]);
        assert_eq!(fb.modifiers(3), Vec::<u64>::new());
    }

    #[test]
    fn a_dev_t_is_read_from_the_first_eight_bytes() {
        assert_eq!(dev_of(&57856u64.to_ne_bytes()), Some(57856));
        assert_eq!(dev_of(&[1, 2]), None);
    }

    #[test]
    fn the_format_table_is_read_from_its_file() {
        use std::io::Write;
        let mut entries = Vec::new();
        for (f, m) in [(0x3432_5241u32, 0u64), (0x3432_5241, 0x0300_0000_00e0_8014)] {
            entries.extend_from_slice(&f.to_ne_bytes());
            entries.extend_from_slice(&[0; 4]);
            entries.extend_from_slice(&m.to_ne_bytes());
        }
        let fd = rustix::fs::memfd_create("table", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
        let mut file = std::fs::File::from(fd);
        file.write_all(&entries).unwrap();
        let table = read_table(file.as_fd().try_clone_to_owned().unwrap(), entries.len());
        assert_eq!(
            table,
            [(0x3432_5241, 0), (0x3432_5241, 0x0300_0000_00e0_8014)]
        );
    }
}

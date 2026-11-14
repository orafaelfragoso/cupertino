//! Shared-memory buffers of a surface: each one is a slot that is free, leased to the client for drawing, or held by
//! the compositor until it releases it.

use crate::Error;
use memmap2::{MmapMut, MmapOptions};
use rustix::fs::{fallocate, memfd_create, FallocateFlags, MemfdFlags};
use std::fs::File;
use std::os::fd::AsFd;
use std::thread::JoinHandle;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::{Dispatch, QueueHandle};

/// The most buffers a surface keeps: one on screen, one the compositor may still read, one to draw into.
pub(crate) const MAX_SLOTS: usize = 3;

/// Pixel formats of a buffer. `wl_shm` requires every compositor to take the first two; the others only where it
/// lists them ([`Connection::offers`](crate::Connection::offers)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferFormat {
    /// Premultiplied alpha; a little-endian 32-bit word, bytes B, G, R, A in memory.
    Argb8888,
    /// Opaque; the fourth byte is ignored.
    Xrgb8888,
    /// Premultiplied alpha; a little-endian 32-bit word, bytes R, G, B, A in memory.
    Abgr8888,
}

impl BufferFormat {
    pub(crate) fn wire(self) -> wl_shm::Format {
        match self {
            BufferFormat::Argb8888 => wl_shm::Format::Argb8888,
            BufferFormat::Xrgb8888 => wl_shm::Format::Xrgb8888,
            BufferFormat::Abgr8888 => wl_shm::Format::Abgr8888,
        }
    }

    /// Whether every compositor takes the format, listed or not.
    pub(crate) fn required(self) -> bool {
        matches!(self, BufferFormat::Argb8888 | BufferFormat::Xrgb8888)
    }
}

/// Pixel dimensions and byte layout of one buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferLayout {
    width: u32,
    height: u32,
    format: BufferFormat,
}

impl BufferLayout {
    /// The largest width or height: a buffer stays within 1 GiB, and a surface's slots within 3 GiB.
    pub const MAX_EXTENT: u32 = 16384;

    /// `None` for an empty buffer or one wider or taller than [`Self::MAX_EXTENT`].
    pub fn new(width: u32, height: u32, format: BufferFormat) -> Option<Self> {
        let extent = 1..=Self::MAX_EXTENT;
        (extent.contains(&width) && extent.contains(&height)).then_some(Self {
            width,
            height,
            format,
        })
    }

    pub fn width(self) -> u32 {
        self.width
    }

    pub fn height(self) -> u32 {
        self.height
    }

    pub fn format(self) -> BufferFormat {
        self.format
    }

    /// Bytes per row.
    pub fn stride(self) -> usize {
        self.width as usize * 4
    }

    /// Bytes of the buffer.
    pub fn buffer_len(self) -> usize {
        self.stride() * self.height as usize
    }
}

/// A rectangle of buffer pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PixelRect {
    /// The part inside a buffer of `layout`, `None` when nothing is.
    pub(crate) fn clamp(self, layout: BufferLayout) -> Option<PixelRect> {
        let (x1, y1) = (
            self.x.saturating_add(self.width).min(layout.width()),
            self.y.saturating_add(self.height).min(layout.height()),
        );
        (self.x < x1 && self.y < y1).then(|| PixelRect {
            x: self.x,
            y: self.y,
            width: x1 - self.x,
            height: y1 - self.y,
        })
    }
}

/// More rectangles than this make a buffer miss their bounds instead.
const MAX_MISSED: usize = 32;

/// What a buffer misses of the frames shown since it last held one: `None` when its pixels are unknown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Missed(Option<Vec<PixelRect>>);

impl Missed {
    pub(crate) fn rects(&self) -> Option<&[PixelRect]> {
        self.0.as_deref()
    }

    /// The buffer holds the frame just shown.
    pub(crate) fn shown(&mut self) {
        self.0 = Some(Vec::new());
    }

    /// Another buffer of the same layout showed a frame that changed `damage`; `None` for all of it. Past
    /// [`MAX_MISSED`] rectangles the buffer misses their bounds, so its other pixels stay known.
    pub(crate) fn passed(&mut self, damage: Option<&[PixelRect]>) {
        self.0 = match (self.0.take(), damage) {
            (Some(mut missed), Some(rects)) => {
                missed.extend_from_slice(rects);
                if missed.len() > MAX_MISSED {
                    missed = vec![bounds(&missed)];
                }
                Some(missed)
            }
            _ => None,
        };
    }

    pub(crate) fn forget(&mut self) {
        self.0 = None;
    }
}

/// The rectangle around every one of `rects`, which are not empty.
fn bounds(rects: &[PixelRect]) -> PixelRect {
    let (x0, y0, x1, y1) = rects
        .iter()
        .fold((u32::MAX, u32::MAX, 0, 0), |(x0, y0, x1, y1), r| {
            (
                x0.min(r.x),
                y0.min(r.y),
                x1.max(r.x + r.width),
                y1.max(r.y + r.height),
            )
        });
    PixelRect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }
}

/// The shared memory of one buffer: the file the compositor maps and the client's own map of it.
pub(crate) struct Memory {
    file: File,
    map: MmapMut,
}

impl Memory {
    /// `len` bytes of zeros, whose pages are made as they are first written.
    fn new(len: usize) -> Result<Memory, Error> {
        let file = Self::file(len)?;
        // SAFETY: the memfd is private to this process and the compositor; neither truncates it.
        let map = unsafe { MmapMut::map_mut(&file) }.map_err(Error::Io)?;
        Ok(Memory { file, map })
    }

    /// `len` bytes of zeros with every page made and mapped already, so that writing the first frame into it costs
    /// what writing into a buffer drawn before does: making the pages of a buffer the size of a large display costs
    /// several times more than writing all of its pixels.
    fn prepared(len: usize) -> Result<Memory, Error> {
        let file = Self::file(len)?;
        // a file system without it makes the pages as they are mapped below
        let _ = fallocate(&file, FallocateFlags::empty(), 0, len as u64);
        // SAFETY: as in `new`.
        let map = unsafe { MmapOptions::new().populate().map_mut(&file) }.map_err(Error::Io)?;
        Ok(Memory { file, map })
    }

    fn file(len: usize) -> Result<File, Error> {
        let fd = memfd_create("cupertino-wayland-client", MemfdFlags::CLOEXEC)
            .map_err(|e| Error::Io(e.into()))?;
        let file = File::from(fd);
        file.set_len(len as u64).map_err(Error::Io)?;
        Ok(file)
    }
}

/// The memory of a surface's next buffer, made on a thread of its own while the surface draws into the buffers it
/// has: a surface that draws frame after frame comes to need one buffer more than it has, and making that buffer on
/// the frame that needs it would cost the frame the pages of a whole buffer.
pub(crate) struct Spare {
    len: usize,
    making: JoinHandle<Result<Memory, Error>>,
}

impl Spare {
    /// Starts making the memory of a buffer of `layout`; `None` when no thread can be started.
    pub(crate) fn start(layout: BufferLayout) -> Option<Spare> {
        let len = layout.buffer_len();
        std::thread::Builder::new()
            .name("cupertino-shm-spare".into())
            .spawn(move || Memory::prepared(len))
            .ok()
            .map(|making| Spare { len, making })
    }

    /// Whether the memory is that of a buffer of `layout`.
    pub(crate) fn fits(&self, layout: BufferLayout) -> bool {
        self.len == layout.buffer_len()
    }

    /// The memory, waiting for it if it is still being made; `None` when it could not be made.
    pub(crate) fn take(self) -> Option<Memory> {
        self.making.join().ok().and_then(Result::ok)
    }
}

/// Identifies a `wl_buffer` in its release event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlotTag {
    pub surface: u32,
    pub slot: u64,
}

/// One buffer with its own pool, so buffers of different sizes coexist while a resize settles.
pub(crate) struct Slot {
    pub tag: SlotTag,
    pub layout: BufferLayout,
    pub busy: bool,
    pub missed: Missed,
    map: MmapMut,
    pool: WlShmPool,
    pub buffer: WlBuffer,
}

impl Slot {
    /// A buffer of `layout` in `memory` (of its size), or in memory made now.
    pub(crate) fn new<D>(
        shm: &WlShm,
        qh: &QueueHandle<D>,
        tag: SlotTag,
        layout: BufferLayout,
        memory: Option<Memory>,
    ) -> Result<Self, Error>
    where
        D: Dispatch<WlShmPool, ()> + Dispatch<WlBuffer, SlotTag> + 'static,
    {
        let Memory { file, map } = match memory {
            Some(memory) if memory.map.len() == layout.buffer_len() => memory,
            _ => Memory::new(layout.buffer_len())?,
        };
        let pool = shm.create_pool(file.as_fd(), layout.buffer_len() as i32, qh, ());
        let buffer = pool.create_buffer(
            0,
            layout.width() as i32,
            layout.height() as i32,
            layout.stride() as i32,
            layout.format().wire(),
            qh,
            tag,
        );
        Ok(Self {
            tag,
            layout,
            busy: false,
            missed: Missed::default(),
            map,
            pool,
            buffer,
        })
    }

    pub(crate) fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.map[..]
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
    }
}

/// Where the next frame of a surface is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotChoice {
    /// This free slot already has the layout.
    Reuse(usize),
    /// A new slot, after the free slots of other layouts are dropped.
    Create,
    /// The compositor holds [`MAX_SLOTS`] buffers.
    AllBusy,
}

/// The choice for a frame of `layout` among slots given as `(busy, layout)`.
pub(crate) fn pick_slot(
    slots: impl Iterator<Item = (bool, BufferLayout)> + Clone,
    layout: BufferLayout,
) -> SlotChoice {
    if let Some(i) = slots.clone().position(|(busy, l)| !busy && l == layout) {
        SlotChoice::Reuse(i)
    } else if slots.filter(|(busy, _)| *busy).count() >= MAX_SLOTS {
        SlotChoice::AllBusy
    } else {
        SlotChoice::Create
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_four_bytes_per_pixel() {
        let l = BufferLayout::new(360, 100, BufferFormat::Argb8888).unwrap();
        assert_eq!((l.stride(), l.buffer_len()), (1440, 144_000));
    }

    #[test]
    fn empty_or_oversized_layouts_are_rejected() {
        let f = BufferFormat::Xrgb8888;
        assert_eq!(BufferLayout::new(0, 10, f), None);
        assert_eq!(BufferLayout::new(10, 0, f), None);
        assert_eq!(BufferLayout::new(16_385, 1, f), None);
        assert_eq!(BufferLayout::new(1, 16_385, f), None);
        assert_eq!(
            BufferLayout::new(16_384, 16_384, f).map(BufferLayout::buffer_len),
            Some(1 << 30)
        );
    }

    #[test]
    fn a_free_slot_of_the_layout_is_reused_and_a_resize_makes_a_new_one() {
        let f = BufferFormat::Argb8888;
        let (small, big) = (
            BufferLayout::new(320, 240, f).unwrap(),
            BufferLayout::new(3840, 2160, f).unwrap(),
        );
        let slots = [(true, small), (false, small)];
        assert_eq!(pick_slot(slots.into_iter(), small), SlotChoice::Reuse(1));
        assert_eq!(pick_slot(slots.into_iter(), big), SlotChoice::Create);
        let held = [(true, small), (true, small), (true, big)];
        assert_eq!(pick_slot(held.into_iter(), big), SlotChoice::AllBusy);
    }

    #[test]
    fn rectangles_are_clamped_to_the_buffer() {
        let l = BufferLayout::new(40, 30, BufferFormat::Argb8888).unwrap();
        let r = |x, y, width, height| PixelRect {
            x,
            y,
            width,
            height,
        };
        assert_eq!(r(30, 20, 20, 20).clamp(l), Some(r(30, 20, 10, 10)));
        assert_eq!(r(40, 0, 5, 5).clamp(l), None);
        assert_eq!(r(0, 0, 0, 5).clamp(l), None);
    }

    #[test]
    fn a_buffer_misses_what_the_frames_shown_elsewhere_changed_until_it_is_shown() {
        let r = |x| PixelRect {
            x,
            y: 0,
            width: 1,
            height: 1,
        };
        let mut missed = Missed::default();
        assert_eq!(missed.rects(), None, "a new buffer");
        missed.passed(Some(&[r(1)]));
        assert_eq!(missed.rects(), None, "unknown stays unknown");
        missed.shown();
        assert_eq!(missed.rects(), Some(&[][..]));
        missed.passed(Some(&[r(1)]));
        missed.passed(Some(&[r(2), r(3)]));
        assert_eq!(missed.rects(), Some(&[r(1), r(2), r(3)][..]));
        missed.passed(None);
        assert_eq!(missed.rects(), None, "a whole frame passed it by");
        missed.shown();
        missed.passed(Some(&vec![r(0); MAX_MISSED]));
        assert_eq!(missed.rects().map(<[_]>::len), Some(MAX_MISSED));
        missed.passed(Some(&[r(5)]));
        let around = PixelRect {
            x: 0,
            y: 0,
            width: 6,
            height: 1,
        };
        assert_eq!(
            missed.rects(),
            Some(&[around][..]),
            "too many rectangles become their bounds"
        );
    }

    #[test]
    fn a_spare_buffer_s_memory_is_made_whole_and_zeroed_on_its_own_thread() {
        let layout = BufferLayout::new(64, 48, BufferFormat::Abgr8888).unwrap();
        let spare = Spare::start(layout).unwrap();
        assert!(spare.fits(layout));
        assert!(!spare.fits(BufferLayout::new(64, 49, BufferFormat::Abgr8888).unwrap()));
        let memory = spare.take().unwrap();
        assert_eq!(memory.map.len(), layout.buffer_len());
        assert!(memory.map.iter().all(|&b| b == 0));
        assert_eq!(
            memory.file.metadata().unwrap().len(),
            layout.buffer_len() as u64
        );
    }
}

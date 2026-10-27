//! Shared-memory buffers of a surface: each one is a slot that is free, leased to the client for drawing, or held by
//! the compositor until it releases it.

use crate::Error;
use memmap2::MmapMut;
use rustix::fs::{memfd_create, MemfdFlags};
use std::fs::File;
use std::os::fd::AsFd;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::{Dispatch, QueueHandle};

/// The most buffers a surface keeps: one on screen, one the compositor may still read, one to draw into.
pub(crate) const MAX_SLOTS: usize = 3;

/// Pixel formats every compositor supports (`wl_shm` requires both).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferFormat {
    /// Premultiplied alpha; a little-endian 32-bit word, bytes B, G, R, A in memory.
    Argb8888,
    /// Opaque; the fourth byte is ignored.
    Xrgb8888,
}

impl BufferFormat {
    fn wire(self) -> wl_shm::Format {
        match self {
            BufferFormat::Argb8888 => wl_shm::Format::Argb8888,
            BufferFormat::Xrgb8888 => wl_shm::Format::Xrgb8888,
        }
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
        (self.x < x1 && self.y < y1).then_some(PixelRect {
            x: self.x,
            y: self.y,
            width: x1 - self.x,
            height: y1 - self.y,
        })
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
    map: MmapMut,
    pool: WlShmPool,
    pub buffer: WlBuffer,
}

impl Slot {
    pub(crate) fn new<D>(
        shm: &WlShm,
        qh: &QueueHandle<D>,
        tag: SlotTag,
        layout: BufferLayout,
    ) -> Result<Self, Error>
    where
        D: Dispatch<WlShmPool, ()> + Dispatch<WlBuffer, SlotTag> + 'static,
    {
        let fd = memfd_create("cupertino-wayland-client", MemfdFlags::CLOEXEC)
            .map_err(|e| Error::Io(e.into()))?;
        let file = File::from(fd);
        file.set_len(layout.buffer_len() as u64)
            .map_err(Error::Io)?;
        // SAFETY: the memfd is private to this process and the compositor; neither truncates it.
        let map = unsafe { MmapMut::map_mut(&file) }.map_err(Error::Io)?;
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
}

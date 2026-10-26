//! Double-buffered shared-memory pixel buffers.

use crate::{ClientError, State, SurfaceId};
use memmap2::MmapMut;
use rustix::fs::{memfd_create, MemfdFlags};
use std::fs::File;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_shm::{Format, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::QueueHandle;

// `wl_shm::Format::Argb8888` is a little-endian 32-bit word, bytes B, G, R, A in memory
const _: () = assert!(cfg!(target_endian = "little"));

/// Number of buffers per surface: one on screen, one to draw into.
const SLOTS: usize = 2;

/// Pixel dimensions and byte layout of one ARGB8888 buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferLayout {
    width: u32,
    height: u32,
}

impl BufferLayout {
    /// `None` for an empty buffer or one whose pool would exceed what `wl_shm` addresses (`i32`).
    pub fn new(width: u32, height: u32) -> Option<Self> {
        let bytes = u64::from(width) * u64::from(height) * 4 * SLOTS as u64;
        (width > 0 && height > 0 && bytes <= i32::MAX as u64).then_some(Self { width, height })
    }

    pub fn width(self) -> u32 {
        self.width
    }

    pub fn height(self) -> u32 {
        self.height
    }

    /// Bytes per row.
    pub fn stride(self) -> usize {
        self.width as usize * 4
    }

    /// Bytes of one buffer.
    pub fn buffer_len(self) -> usize {
        self.stride() * self.height as usize
    }

    /// Bytes of the pool holding every slot.
    pub fn pool_len(self) -> usize {
        self.buffer_len() * SLOTS
    }

    /// Byte offset of a slot inside the pool.
    pub fn slot_offset(self, slot: usize) -> usize {
        self.buffer_len() * slot
    }
}

/// Convert premultiplied RGBA bytes (the layout of `tiny_skia::Pixmap`) to premultiplied
/// ARGB8888 (`wl_shm`) by swapping the red and blue bytes of every pixel.
pub fn rgba_to_argb8888(rgba: &[u8], argb: &mut [u8]) {
    assert_eq!(rgba.len(), argb.len(), "pixel buffers differ in size");
    let (src, _) = rgba.as_chunks::<4>();
    let (dst, _) = argb.as_chunks_mut::<4>();
    for (s, d) in src.iter().zip(dst) {
        *d = [s[2], s[1], s[0], s[3]];
    }
}

/// Identifies a `wl_buffer` in its release event: the surface, the pool generation (a pool is
/// replaced when the buffer size changes) and the slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BufferTag {
    pub surface: SurfaceId,
    pub generation: u64,
    pub slot: usize,
}

/// One pool with [`SLOTS`] buffers of a surface.
pub(crate) struct BufferPool {
    layout: BufferLayout,
    generation: u64,
    pool: WlShmPool,
    map: MmapMut,
    buffers: Vec<WlBuffer>,
    busy: [bool; SLOTS],
}

impl BufferPool {
    pub(crate) fn new(
        shm: &WlShm,
        qh: &QueueHandle<State>,
        surface: SurfaceId,
        generation: u64,
        layout: BufferLayout,
    ) -> Result<Self, ClientError> {
        let fd = memfd_create("cupertino-client", MemfdFlags::CLOEXEC)
            .map_err(|e| ClientError::Shm(e.into()))?;
        let file = File::from(fd);
        file.set_len(layout.pool_len() as u64)
            .map_err(ClientError::Shm)?;
        // SAFETY: the memfd is private to this process and the server; neither truncates it.
        let map = unsafe { MmapMut::map_mut(&file) }.map_err(ClientError::Shm)?;
        let pool = shm.create_pool(
            std::os::fd::AsFd::as_fd(&file),
            layout.pool_len() as i32,
            qh,
            (),
        );
        let buffers = (0..SLOTS)
            .map(|slot| {
                let tag = BufferTag {
                    surface,
                    generation,
                    slot,
                };
                pool.create_buffer(
                    layout.slot_offset(slot) as i32,
                    layout.width() as i32,
                    layout.height() as i32,
                    layout.stride() as i32,
                    Format::Argb8888,
                    qh,
                    tag,
                )
            })
            .collect();
        Ok(Self {
            layout,
            generation,
            pool,
            map,
            buffers,
            busy: [false; SLOTS],
        })
    }

    pub(crate) fn layout(&self) -> BufferLayout {
        self.layout
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn has_free_slot(&self) -> bool {
        self.busy.iter().any(|b| !b)
    }

    pub(crate) fn release(&mut self, slot: usize) {
        self.busy[slot] = false;
    }

    /// Fill a free slot from premultiplied RGBA pixels and mark it busy until the server
    /// releases it. `None` when both buffers are still in use.
    pub(crate) fn fill(&mut self, rgba: &[u8]) -> Option<&WlBuffer> {
        let slot = self.busy.iter().position(|b| !b)?;
        let start = self.layout.slot_offset(slot);
        rgba_to_argb8888(rgba, &mut self.map[start..start + self.layout.buffer_len()]);
        self.busy[slot] = true;
        Some(&self.buffers[slot])
    }
}

impl Drop for BufferPool {
    fn drop(&mut self) {
        for b in &self.buffers {
            b.destroy();
        }
        self.pool.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_four_bytes_per_pixel_with_two_slots() {
        let l = BufferLayout::new(360, 100).unwrap();
        assert_eq!(
            (l.stride(), l.buffer_len(), l.pool_len()),
            (1440, 144_000, 288_000)
        );
        assert_eq!((l.slot_offset(0), l.slot_offset(1)), (0, 144_000));
    }

    #[test]
    fn empty_or_oversized_layouts_are_rejected() {
        assert_eq!(BufferLayout::new(0, 10), None);
        assert_eq!(BufferLayout::new(10, 0), None);
        assert_eq!(BufferLayout::new(20_000, 20_000), None);
    }

    #[test]
    fn rgba_becomes_argb8888_bytes() {
        let rgba = [10, 20, 30, 40, 1, 2, 3, 255];
        let mut argb = [0; 8];
        rgba_to_argb8888(&rgba, &mut argb);
        assert_eq!(argb, [30, 20, 10, 40, 3, 2, 1, 255]);
    }
}

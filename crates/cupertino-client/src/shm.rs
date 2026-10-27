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

/// A rectangle of device pixels.
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

/// What a frame changed: everything, or these rectangles (inside the buffer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Damage {
    Full,
    Rects(Vec<PixelRect>),
}

/// More pending rectangles than this make a slot take the whole frame.
const MAX_PENDING: usize = 32;

/// What each slot missed since it was last written: `None` when its content is unknown (never written, or a full frame
/// passed it by), else the rectangles of the frames that went to the other slot.
#[derive(Clone, Debug, Default)]
pub(crate) struct SlotDamage {
    missed: [Option<Vec<PixelRect>>; SLOTS],
}

impl SlotDamage {
    /// The regions to copy into `slot` for a frame that changed `damage`, and the record of what the other slots
    /// now miss.
    pub(crate) fn take(&mut self, slot: usize, damage: &Damage) -> Damage {
        let copy = match (damage, self.missed[slot].take()) {
            (Damage::Rects(rects), Some(mut missed)) => {
                missed.extend_from_slice(rects);
                Damage::Rects(missed)
            }
            _ => Damage::Full,
        };
        self.missed[slot] = Some(Vec::new());
        for (other, missed) in self.missed.iter_mut().enumerate() {
            if other == slot {
                continue;
            }
            *missed = match (damage, missed.take()) {
                (Damage::Rects(rects), Some(mut m)) if m.len() + rects.len() <= MAX_PENDING => {
                    m.extend_from_slice(rects);
                    Some(m)
                }
                _ => None,
            };
        }
        copy
    }
}

/// Convert the `regions` of the premultiplied RGBA frame `rgba` into the ARGB8888 buffer `argb`, both of `layout`.
pub(crate) fn copy_regions(rgba: &[u8], argb: &mut [u8], layout: BufferLayout, regions: &Damage) {
    let rects = match regions {
        Damage::Full => return rgba_to_argb8888(rgba, argb),
        Damage::Rects(rects) => rects,
    };
    let stride = layout.stride();
    for r in rects {
        let Some(r) = r.clamp(layout) else {
            continue;
        };
        let (start, len) = (r.x as usize * 4, r.width as usize * 4);
        for row in r.y..r.y + r.height {
            let at = row as usize * stride + start;
            rgba_to_argb8888(&rgba[at..at + len], &mut argb[at..at + len]);
        }
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
    damage: SlotDamage,
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
            damage: SlotDamage::default(),
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

    /// Bring a free slot up to the premultiplied RGBA frame `rgba`, which changed `damage` since the last one: only
    /// what changed and what the slot missed is converted. The slot is busy until the server releases it; `None` when
    /// both buffers are still in use.
    pub(crate) fn fill(&mut self, rgba: &[u8], damage: &Damage) -> Option<&WlBuffer> {
        let slot = self.busy.iter().position(|b| !b)?;
        let start = self.layout.slot_offset(slot);
        let regions = self.damage.take(slot, damage);
        copy_regions(
            rgba,
            &mut self.map[start..start + self.layout.buffer_len()],
            self.layout,
            &regions,
        );
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

    /// A frame of `layout` with a value of its own in every byte, changed by `seed` inside `damage`.
    fn repaint(frame: &mut [u8], layout: BufferLayout, damage: &Damage, seed: u8) {
        let all = [PixelRect {
            x: 0,
            y: 0,
            width: layout.width(),
            height: layout.height(),
        }];
        let rects = match damage {
            Damage::Full => &all[..],
            Damage::Rects(r) => &r[..],
        };
        for r in rects.iter().filter_map(|r| r.clamp(layout)) {
            for y in r.y..r.y + r.height {
                for x in r.x..r.x + r.width {
                    let at = (y as usize * layout.width() as usize + x as usize) * 4;
                    frame[at..at + 4].copy_from_slice(&[seed, x as u8, y as u8, 255 - seed]);
                }
            }
        }
    }

    #[test]
    fn every_presented_slot_equals_a_full_conversion_of_its_frame() {
        let layout = BufferLayout::new(40, 30).unwrap();
        let r = |x, y, width, height| PixelRect {
            x,
            y,
            width,
            height,
        };
        // (damage, slot the free one is): the server sometimes holds a buffer, so a slot is reused twice in a row
        let frames = [
            (Damage::Full, 0),
            (Damage::Rects(vec![r(2, 3, 5, 4)]), 1),
            (Damage::Rects(vec![r(30, 20, 20, 20)]), 0),
            (Damage::Rects(vec![r(0, 0, 1, 1), r(10, 10, 3, 3)]), 0),
            (Damage::Rects(vec![r(5, 25, 30, 2)]), 1),
            (Damage::Full, 1),
            (Damage::Rects(vec![r(39, 29, 1, 1)]), 0),
            (Damage::Rects(vec![r(12, 0, 4, 30)]), 1),
            (Damage::Rects(vec![]), 0),
            (Damage::Rects(vec![r(1, 1, 2, 2)]), 1),
        ];
        let mut frame = vec![0u8; layout.buffer_len()];
        let mut slots = vec![vec![0u8; layout.buffer_len()]; SLOTS];
        let mut tracker = SlotDamage::default();
        for (i, (damage, slot)) in frames.iter().enumerate() {
            repaint(&mut frame, layout, damage, i as u8 + 1);
            let regions = tracker.take(*slot, damage);
            copy_regions(&frame, &mut slots[*slot], layout, &regions);
            let mut full = vec![0u8; layout.buffer_len()];
            rgba_to_argb8888(&frame, &mut full);
            assert!(slots[*slot] == full, "frame {i} in slot {slot}");
        }
    }

    #[test]
    fn a_slot_that_missed_too_many_rectangles_takes_the_whole_frame() {
        let mut tracker = SlotDamage::default();
        tracker.take(0, &Damage::Full);
        tracker.take(1, &Damage::Full);
        let one = Damage::Rects(vec![PixelRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }]);
        for _ in 0..MAX_PENDING {
            tracker.take(0, &one);
        }
        assert!(matches!(tracker.take(1, &one), Damage::Rects(r) if r.len() == MAX_PENDING + 1));
        for _ in 0..=MAX_PENDING {
            tracker.take(0, &one);
        }
        assert_eq!(tracker.take(1, &one), Damage::Full);
    }

    #[test]
    fn a_damaged_frame_converts_its_rectangles_whatever_the_buffer_size() {
        // a caret and a toolbar strip
        let damage = Damage::Rects(vec![
            PixelRect {
                x: 300,
                y: 20,
                width: 2,
                height: 26,
            },
            PixelRect {
                x: 0,
                y: 0,
                width: 600,
                height: 80,
            },
        ]);
        for (w, h) in [(600, 400), (1920, 1080), (3840, 2160)] {
            let layout = BufferLayout::new(w, h).unwrap();
            let rgba = vec![7u8; layout.buffer_len()];
            let mut argb = vec![0u8; layout.buffer_len()];
            let mut best = |regions: &Damage| {
                (0..3)
                    .map(|_| {
                        let start = std::time::Instant::now();
                        copy_regions(&rgba, &mut argb, layout, regions);
                        start.elapsed()
                    })
                    .min()
                    .unwrap()
            };
            // the whole buffer first: its pages are touched before the damage is timed
            let full = best(&Damage::Full);
            let damaged = best(&damage);
            eprintln!("{w} x {h}: the damage {damaged:?}, the whole buffer {full:?}");
            assert!(
                damaged < std::time::Duration::from_millis(20),
                "{w} x {h}: {damaged:?}"
            );
            if w >= 1920 {
                assert!(
                    damaged * 10 < full,
                    "{w} x {h}: {damaged:?} against {full:?}"
                );
            }
        }
    }
}

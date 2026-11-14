//! CPU-drawn images as scene elements: a cache of premultiplied pixmaps turned into memory buffers
//! (re-uploaded only when their signature changes) and the element constructor that places one at a
//! logical position. Shared by the input-source HUD and the accent popup.

use std::collections::HashMap;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Physical, Point, Rectangle, Size, Transform};
use tiny_skia::Pixmap;

use super::CupertinoElement;

struct Cached {
    sig: u64,
    buffer: MemoryRenderBuffer,
    px: (i32, i32),
    extra: [u32; 4],
    used: bool,
}

/// A cached image: buffer, size in device pixels, margins.
pub type Cut = (MemoryRenderBuffer, (i32, i32), [u32; 4]);

/// Memory buffers by key; entries not used during a frame are dropped by [`ImageCache::end_frame`].
#[derive(Default)]
pub struct ImageCache {
    map: HashMap<(&'static str, u64), Cached>,
    /// Images drawn on the CPU so far (the frame statistics count them per frame).
    pub rasters: u64,
}

impl ImageCache {
    /// Buffer for `key`; `render` runs only when `sig` differs from the cached one. It returns the pixmap and
    /// four device-pixel margins (left, top, right, bottom: space around the content that is not part of
    /// it, e.g. shadow room), handed back unchanged.
    pub fn get(
        &mut self,
        key: (&'static str, u64),
        sig: u64,
        render: impl FnOnce() -> Option<(Pixmap, [u32; 4])>,
    ) -> Option<Cut> {
        if let Some(c) = self.map.get_mut(&key) {
            if c.sig == sig {
                c.used = true;
                return Some((c.buffer.clone(), c.px, c.extra));
            }
        }
        let (pm, extra) = render()?;
        self.rasters += 1;
        let px = (pm.width() as i32, pm.height() as i32);
        let buffer = MemoryRenderBuffer::from_slice(
            pm.data(),
            Fourcc::Abgr8888,
            px,
            1,
            Transform::Normal,
            None,
        );
        self.map.insert(
            key,
            Cached {
                sig,
                buffer: buffer.clone(),
                px,
                extra,
                used: true,
            },
        );
        Some((buffer, px, extra))
    }

    pub fn end_frame(&mut self) {
        self.map
            .retain(|_, c| std::mem::replace(&mut c.used, false));
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Element drawing `buf` (`px` device pixels) with its top-left corner at output-local logical
/// `(x, y)`; the logical size is the pixel size divided by `scale`.
pub fn image_element(
    renderer: &mut GlesRenderer,
    buf: &MemoryRenderBuffer,
    px: (i32, i32),
    x: f64,
    y: f64,
    scale: f64,
    alpha: f32,
) -> Option<CupertinoElement> {
    let size = Size::<i32, Logical>::from((
        (f64::from(px.0) / scale).round() as i32,
        (f64::from(px.1) / scale).round() as i32,
    ));
    let src = Rectangle::<f64, Logical>::new(
        (0.0, 0.0).into(),
        (f64::from(px.0), f64::from(px.1)).into(),
    );
    MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        Point::<f64, Physical>::from((x * scale, y * scale)),
        buf,
        Some(alpha),
        Some(src),
        Some(size),
        Kind::Unspecified,
    )
    .ok()
    .map(CupertinoElement::from)
}

/// Hash of anything `Debug` (cache signatures of structured state).
pub fn debug_sig<T: std::fmt::Debug>(v: &T) -> u64 {
    use std::fmt::Write;
    use std::hash::Hasher;
    /// Hashes what is written into it, so the text is never kept.
    struct Hashing(std::collections::hash_map::DefaultHasher);
    impl Write for Hashing {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.0.write(s.as_bytes());
            Ok(())
        }
    }
    let mut h = Hashing(std::collections::hash_map::DefaultHasher::new());
    // writing into the hasher cannot fail
    let _ = write!(h, "{v:?}");
    h.0.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_decides_re_rendering() {
        let mut c = ImageCache::default();
        let mut renders = 0;
        let mut get = |c: &mut ImageCache, sig| {
            c.get(("t", 0), sig, || {
                renders += 1;
                Pixmap::new(4, 3).map(|p| (p, [1, 2, 3, 4]))
            })
            .map(|(_, px, extra)| {
                assert_eq!(extra, [1, 2, 3, 4]);
                px
            })
        };
        assert_eq!(get(&mut c, 1), Some((4, 3)));
        assert_eq!(get(&mut c, 1), Some((4, 3)));
        assert_eq!(get(&mut c, 2), Some((4, 3)));
        assert_eq!(renders, 2);
        c.end_frame();
        assert_eq!(c.len(), 1);
        c.end_frame();
        assert!(c.is_empty(), "unused entries are dropped");
    }

    #[test]
    fn debug_sig_is_stable_and_content_based() {
        assert_eq!(debug_sig(&("a", 1)), debug_sig(&("a", 1)));
        assert_ne!(debug_sig(&("a", 1)), debug_sig(&("a", 2)));
    }
}

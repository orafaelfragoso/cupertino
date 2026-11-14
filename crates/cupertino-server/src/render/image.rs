//! Images of the keyboard HUDs (the input-source HUD, the shortcut notice, the accent popup) as scene elements: a
//! cache of their lists of draws, recorded again only when their signature changes and drawn on the GPU like the
//! chrome ([`super::chrome`]); and the element constructor that places a CPU-drawn buffer at a logical position.

use std::collections::HashMap;
use std::rc::Rc;

use cupertino_decorations::DrawList;

use super::chrome::ChromeImage;
use super::uploads::Drawer;
use super::CupertinoElement;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Logical, Physical, Point, Rectangle, Size};

struct Cached {
    sig: u64,
    image: Rc<ChromeImage>,
    used: bool,
}

/// A cached image and its size in device pixels.
pub type Cut = (Rc<ChromeImage>, (i32, i32));

/// HUD images by key; entries not used during a frame are dropped by [`ImageCache::end_frame`].
#[derive(Default)]
pub struct ImageCache {
    map: HashMap<(&'static str, u64), Cached>,
    /// Lists of draws recorded so far (diagnostics and tests).
    pub records: u64,
}

impl ImageCache {
    /// The image of `key`; `record` runs only when `sig` differs from the cached one, and the image there takes its
    /// list (keeping the textures it was drawn into).
    pub fn get(
        &mut self,
        key: (&'static str, u64),
        sig: u64,
        record: impl FnOnce() -> Option<DrawList>,
    ) -> Option<Cut> {
        if let Some(c) = self.map.get_mut(&key) {
            if c.sig == sig {
                c.used = true;
                return Some((c.image.clone(), c.image.size()));
            }
        }
        let list = record()?;
        self.records += 1;
        let image = match self.map.get_mut(&key) {
            Some(c) => {
                c.image.set(list);
                c.sig = sig;
                c.used = true;
                c.image.clone()
            }
            None => {
                let image = ChromeImage::new(list, Drawer::Hud);
                self.map.insert(
                    key,
                    Cached {
                        sig,
                        image: image.clone(),
                        used: true,
                    },
                );
                image
            }
        };
        Some((image.clone(), image.size()))
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
                Some(DrawList::new(4, 3))
            })
            .map(|(_, px)| px)
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

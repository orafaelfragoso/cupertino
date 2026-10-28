use crate::{ImageResource, PixelFormat, RenderError, ResourceId};
use std::collections::HashMap;
use std::sync::Arc;
use tiny_skia::Pixmap;

struct Entry {
    pixmap: Arc<Pixmap>,
    used: u64,
}

/// Decoded, premultiplied images by resource identity and revision.
pub(crate) struct ImageCache {
    entries: HashMap<(ResourceId, u64), Entry>,
    bytes: usize,
    budget: usize,
    tick: u64,
}

impl ImageCache {
    pub(crate) fn new(budget: usize) -> ImageCache {
        ImageCache {
            entries: HashMap::new(),
            bytes: 0,
            budget,
            tick: 0,
        }
    }

    pub(crate) fn get(
        &mut self,
        id: ResourceId,
        image: &ImageResource,
        max_pixels: u64,
    ) -> Result<Arc<Pixmap>, RenderError> {
        self.tick += 1;
        let key = (id, image.revision);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.used = self.tick;
            return Ok(Arc::clone(&entry.pixmap));
        }
        if u64::from(image.width) * u64::from(image.height) > max_pixels {
            return Err(RenderError::ResourceLimit);
        }
        let pixmap = Arc::new(premultiplied(image).ok_or(RenderError::ResourceLimit)?);
        self.bytes += pixmap.data().len();
        self.entries.insert(
            key,
            Entry {
                pixmap: Arc::clone(&pixmap),
                used: self.tick,
            },
        );
        Ok(pixmap)
    }

    /// Between frames, like the glyph cache.
    pub(crate) fn trim(&mut self) {
        if self.bytes <= self.budget {
            return;
        }
        let mut order: Vec<(u64, (ResourceId, u64))> =
            self.entries.iter().map(|(k, e)| (e.used, *k)).collect();
        order.sort_unstable_by_key(|(used, _)| *used);
        for (_, key) in order {
            if self.bytes <= self.budget {
                break;
            }
            if let Some(entry) = self.entries.remove(&key) {
                self.bytes -= entry.pixmap.data().len();
            }
        }
    }
}

fn premultiplied(image: &ImageResource) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(image.width, image.height)?;
    let row = image.width as usize * 4;
    let stride = image.stride as usize;
    for (y, out) in pixmap.data_mut().chunks_exact_mut(row).enumerate() {
        let src = &image.pixels[y * stride..y * stride + row];
        match image.format {
            PixelFormat::Rgba8Premultiplied => out.copy_from_slice(src),
            PixelFormat::Rgba8 => {
                for (o, s) in out
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(src.as_chunks::<4>().0)
                {
                    let a = u32::from(s[3]);
                    for c in 0..3 {
                        o[c] = crate::effects::div255(u32::from(s[c]) * a) as u8;
                    }
                    o[3] = s[3];
                }
            }
        }
    }
    Some(pixmap)
}

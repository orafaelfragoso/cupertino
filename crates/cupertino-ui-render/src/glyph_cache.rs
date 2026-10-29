use crate::RenderError;
use crate::Tally;
use cupertino_ui_text::{GlyphKey, GlyphRasterizer, RasterGlyph};
use std::collections::HashMap;

struct Entry {
    glyph: Option<RasterGlyph>,
    used: u64,
}

/// Rasterized glyphs by their full raster identity, evicted least recently used between frames.
pub(crate) struct GlyphCache {
    entries: HashMap<GlyphKey, Entry>,
    bytes: usize,
    budget: usize,
    tick: u64,
    /// Entries made since the last [`take_filled`](Self::take_filled).
    filled: Tally,
}

impl GlyphCache {
    pub(crate) fn new(budget: usize) -> GlyphCache {
        GlyphCache {
            entries: HashMap::new(),
            bytes: 0,
            budget,
            tick: 0,
            filled: Tally::default(),
        }
    }

    pub(crate) fn get<R: GlyphRasterizer>(
        &mut self,
        key: GlyphKey,
        rasterizer: &mut R,
    ) -> Result<Option<RasterGlyph>, RenderError> {
        self.tick += 1;
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.used = self.tick;
            return Ok(entry.glyph.clone());
        }
        let glyph = rasterizer.rasterize(&key).map_err(RenderError::Glyph)?;
        let made = glyph.as_ref().map_or(0, |g| g.pixels.len());
        self.filled.add(made as u64);
        self.bytes += made;
        self.entries.insert(
            key,
            Entry {
                glyph: glyph.clone(),
                used: self.tick,
            },
        );
        Ok(glyph)
    }

    pub(crate) fn take_filled(&mut self) -> Tally {
        std::mem::take(&mut self.filled)
    }

    pub(crate) fn retained(&self) -> Tally {
        Tally {
            count: self.entries.len() as u64,
            bytes: self.bytes as u64,
        }
    }

    /// Drops the least recently used glyphs until the cache fits its budget. Called between frames, so no glyph of the
    /// frame being painted is dropped.
    pub(crate) fn trim(&mut self) {
        if self.bytes <= self.budget {
            return;
        }
        let mut order: Vec<(u64, GlyphKey)> =
            self.entries.iter().map(|(k, e)| (e.used, *k)).collect();
        order.sort_unstable_by_key(|(used, _)| *used);
        for (_, key) in order {
            if self.bytes <= self.budget {
                break;
            }
            if let Some(entry) = self.entries.remove(&key) {
                self.bytes -= entry.glyph.map_or(0, |g| g.pixels.len());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_ui_text::{FontId, GlyphFormat, GlyphId, TextError};
    use std::sync::Arc;

    struct Counting(u32);

    impl GlyphRasterizer for Counting {
        fn rasterize(&mut self, _: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
            self.0 += 1;
            Ok(Some(RasterGlyph {
                format: GlyphFormat::Coverage,
                width: 10,
                height: 10,
                stride: 10,
                bearing_x: 0,
                bearing_y: 0,
                pixels: Arc::from(vec![255u8; 100]),
            }))
        }
    }

    fn key(glyph: u32) -> GlyphKey {
        GlyphKey {
            font: FontId(0),
            glyph: GlyphId(glyph),
            size_q: 64,
            subpixel_x: 0,
            dilation_q: 0,
        }
    }

    #[test]
    fn trim_drops_the_least_recently_used_and_keeps_the_rest() {
        let mut cache = GlyphCache::new(200);
        let mut r = Counting(0);
        for g in 0..3 {
            cache.get(key(g), &mut r).unwrap();
        }
        cache.get(key(0), &mut r).unwrap();
        cache.trim();
        assert_eq!(cache.len(), 2);
        cache.get(key(0), &mut r).unwrap();
        assert_eq!(r.0, 3, "glyph 0 was used last and stayed cached");
        cache.get(key(1), &mut r).unwrap();
        assert_eq!(r.0, 4, "glyph 1 was evicted and rasterized again");
    }
}
